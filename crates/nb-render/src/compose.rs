//! Local "Export as PDF": rebuild a note's pages from its edit log, in pure Rust.
//!
//! Notability's own Export as PDF runs client-side in the browser; there is no server job.
//! This is an independent approximation:
//! * page backgrounds: the imported PDF page (source asset + `pageOffset`), or blank paper in the
//!   note/page colour with the template flair (0 lines, 1 dots, 2 grid);
//! * handwriting: `CreateInk` centre paths (+ `AddPathElements`) honouring `ModifyInk`,
//!   `ModifyPositions` and deletes; per-point widths, fill outlines, highlighters above blocks but under ink and shapes;
//! * shapes: polygons, lines, rectangles and ellipses;
//! * typed text: the note body flowed inside the margins from page 1 (styles not reproduced) and
//!   text boxes; images placed at their origin and size (flips honoured, crops not); math blocks as their LaTeX source.
//!
//! Every page carries a small "Local render" footer unless disabled, because fonts, pen textures,
//! styling and layout differ from what Notability itself exports.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use hayro_write::pdf_writer::types::{LineCapStyle, LineJoinStyle};
use hayro_write::pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use hayro_write::{ChunkSettings, ExtractionQuery, extract};
use nb_domain::ids::{Id, SeqId, id_of, seq_of};
use nb_domain::state::{NoteState, bytes_of};
use nb_protocol::inkpath;
use serde_json::{Map, Value, json};

use crate::{PdfError, Result, open};

pub const LABEL: &str =
    "Local render by notability-cli (approximation; not Notability's own export)";
const LINE_RGB: (f32, f32, f32) = (0.69, 0.80, 0.93);
const DOT_RGB: (f32, f32, f32) = (0.62, 0.62, 0.66);
const HIGHLIGHTER: u64 = 2;
const KAPPA: f64 = 0.552_284_75;

type Rgba = (f32, f32, f32, f32);

fn rgb(c: Option<&Value>, default: (f32, f32, f32)) -> Rgba {
    match c.filter(|c| c.is_object() && !c.as_object().is_some_and(Map::is_empty)) {
        None => (default.0, default.1, default.2, 1.0),
        Some(c) => {
            let f = |k: &str, d: f64| (c[k].as_f64().unwrap_or(d) / 255.0) as f32;
            (
                f("bitsR", 0.0),
                f("bitsG", 0.0),
                f("bitsB", 0.0),
                f("bitsA", 255.0),
            )
        }
    }
}

/// Unwrap the `Set*` tables (`{value: x}`) used by Modify ops.
fn unset(v: &Value) -> Value {
    match v.as_object() {
        Some(o) if o.len() == 1 && o.contains_key("value") => o["value"].clone(),
        _ => v.clone(),
    }
}

fn lamport(o: &Value) -> (u64, u64) {
    (
        o["id"]["timestamp"].as_u64().unwrap_or(0),
        o["id"]["site"].as_u64().unwrap_or(0),
    )
}

fn sorted(st: &NoteState) -> Vec<&Value> {
    let mut v: Vec<&Value> = st.ops.iter().collect();
    v.sort_by_key(|o| lamport(o));
    v
}

/// One drawable: `kind` is ink, shape or block; `v` its merged properties.
struct Entity {
    kind: &'static str,
    v: Map<String, Value>,
    extra: Vec<Vec<u8>>,
}

/// Current inks / shapes / blocks keyed by id, with modifications applied (LWW by Lamport).
fn entities(st: &NoteState) -> BTreeMap<Id, Entity> {
    let mut ents: BTreeMap<Id, Entity> = BTreeMap::new();
    for o in sorted(st) {
        let ty = o["payload"]["type"].as_str().unwrap_or_default();
        let v = &o["payload"]["value"];
        let Some(oid) = id_of(&o["id"]) else { continue };
        match ty {
            "CreateInk" | "CreateShape" | "CreateBlock" => {
                let kind = match ty {
                    "CreateInk" => "ink",
                    "CreateShape" => "shape",
                    _ => "block",
                };
                let m = v.as_object().cloned().unwrap_or_default();
                ents.insert(
                    oid,
                    Entity {
                        kind,
                        v: m,
                        extra: Vec::new(),
                    },
                );
            }
            "AddPathElements" => {
                if let (Some(iid), Some(b)) = (
                    id_of(&v["ink"]),
                    v.get("encodedCenterPathElements").and_then(bytes_of),
                ) && let Some(e) = ents.get_mut(&iid)
                {
                    e.extra.push(b);
                }
            }
            "ModifyInk" | "ModifyBlock" | "ModifyShape" => {
                let key = match ty {
                    "ModifyInk" => "inks",
                    "ModifyBlock" => "blocks",
                    _ => "shapes",
                };
                for t in v[key].as_array().into_iter().flatten() {
                    let Some(e) = id_of(t).and_then(|i| ents.get_mut(&i)) else {
                        continue;
                    };
                    for (k, x) in v.as_object().into_iter().flatten() {
                        if k != key {
                            e.v.insert(k.clone(), unset(x));
                        }
                    }
                    if v.get("encodedCenterPath").is_some() {
                        e.extra.clear();
                    }
                }
            }
            "ModifyPositions" => {
                for m in v["modifications"].as_array().into_iter().flatten() {
                    let Some(e) = id_of(&m["target"]).and_then(|i| ents.get_mut(&i)) else {
                        continue;
                    };
                    for k in ["page", "origin", "rotation", "scale", "zIndex"] {
                        if let Some(x) = m.get(k) {
                            e.v.insert(k.into(), unset(x));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let (dels, _) = st.deleted();
    ents.retain(|k, _| !dels.contains(k));
    ents
}

/// page id -> page number inside its background PDF.
fn pdf_page_index(st: &NoteState) -> HashMap<SeqId, usize> {
    let mut out = HashMap::new();
    for o in sorted(st) {
        let v = &o["payload"]["value"];
        match o["payload"]["type"].as_str() {
            Some("CreatePage") => {
                let pdf = &v["background"]["pdf"];
                if pdf.is_object() {
                    let off = pdf["pageOffset"].as_u64().unwrap_or(0) as usize;
                    let (s, t) = (
                        o["id"]["site"].as_u64().unwrap_or(0),
                        o["id"]["timestamp"].as_u64().unwrap_or(0),
                    );
                    for i in 0..v["pageCount"].as_u64().filter(|c| *c > 0).unwrap_or(1) {
                        out.insert((s, t, i), off + i as usize);
                    }
                }
            }
            Some("ModifyPage") => {
                let pdf = &v["background"]["value"]["pdf"];
                if pdf.is_object() {
                    let off = pdf["pageOffset"].as_u64().unwrap_or(0) as usize;
                    for (j, p) in v["pages"].as_array().into_iter().flatten().enumerate() {
                        if let Some(p) = seq_of(p) {
                            out.insert(p, off + j);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn asset_hash(v: &Value) -> Option<String> {
    v["metadata"]["assetHash"].as_str().map(str::to_string)
}

/// Every asset (sha512) the render can use: background PDFs and images.
pub fn needed_assets(st: &NoteState) -> Vec<String> {
    let note_bg = st.note_background();
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: Option<String>| {
        if let Some(s) = s
            && !out.contains(&s)
        {
            out.push(s);
        }
    };
    push(asset_hash(&note_bg["pdf"]));
    for p in st.pages() {
        if let Some(bg) = &p.background {
            push(asset_hash(&bg["pdf"]));
        }
    }
    for e in entities(st).values() {
        if e.kind == "block"
            && let Some(img) = e.v.get("image")
        {
            push(asset_hash(img));
        }
    }
    out
}

// ---------------------------------------------------------------- fonts

#[rustfmt::skip]
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

#[derive(Clone, Copy, PartialEq)]
enum Font {
    Helv,
    Cour,
}

impl Font {
    fn name(self) -> Name<'static> {
        match self {
            Font::Helv => Name(b"F1"),
            Font::Cour => Name(b"F2"),
        }
    }
    fn width(self, s: &str, size: f64) -> f64 {
        let units: u32 = winansi(s)
            .iter()
            .map(|b| match self {
                Font::Cour => 600,
                Font::Helv if (32..127).contains(b) => u32::from(HELV[usize::from(*b - 32)]),
                Font::Helv => 556,
            })
            .sum();
        f64::from(units) * size / 1000.0
    }
}

/// WinAnsi bytes for `s` (unmappable characters become `?`).
fn winansi(s: &str) -> Vec<u8> {
    s.chars()
        .filter(|c| *c != '\r')
        .map(|c| match c {
            '\t' => b' ',
            ' '..='~' => c as u8,
            '\u{a0}'..='\u{ff}' => c as u32 as u8,
            '\u{20ac}' => 0x80,
            '\u{2026}' => 0x85,
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92,
            '\u{201c}' => 0x93,
            '\u{201d}' => 0x94,
            '\u{2022}' => 0x95,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            _ => b'?',
        })
        .collect()
}

fn wrap(s: &str, font: Font, size: f64, width: f64) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ') {
        let trial = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if cur.is_empty() || font.width(&trial, size) <= width {
            cur = trial;
        } else {
            out.push(std::mem::replace(&mut cur, word.to_string()));
        }
    }
    out.push(cur);
    out
}

// ---------------------------------------------------------------- page output

struct PageOut {
    w: f64,
    h: f64,
    /// `Xn` source-page XObject drawn first (unflipped).
    src: Option<usize>,
    c: Content,
}

struct Ctx {
    alphas: Vec<f32>,
    images: Vec<Ref>,
}

impl Ctx {
    fn gs(&mut self, c: &mut Content, a: f32) {
        let a = (a.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        if a >= 0.999 {
            return;
        }
        let i = self.alphas.iter().position(|x| *x == a).unwrap_or_else(|| {
            self.alphas.push(a);
            self.alphas.len() - 1
        });
        c.set_parameters(Name(format!("G{i}").as_bytes()));
    }
}

fn text_at(c: &mut Content, font: Font, size: f64, x: f64, y: f64, s: &str, col: (f32, f32, f32)) {
    c.set_fill_rgb(col.0, col.1, col.2);
    c.begin_text();
    c.set_font(font.name(), size as f32);
    c.set_text_matrix([1.0, 0.0, 0.0, -1.0, x as f32, y as f32]);
    c.show(Str(&winansi(s)));
    c.end_text();
}

fn ellipse(c: &mut Content, x0: f64, y0: f64, x1: f64, y1: f64) {
    let (cx, cy, rx, ry) = (
        (x0 + x1) / 2.0,
        (y0 + y1) / 2.0,
        (x1 - x0) / 2.0,
        (y1 - y0) / 2.0,
    );
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let f = |v: f64| v as f32;
    c.move_to(f(cx + rx), f(cy));
    c.cubic_to(
        f(cx + rx),
        f(cy + ky),
        f(cx + kx),
        f(cy + ry),
        f(cx),
        f(cy + ry),
    );
    c.cubic_to(
        f(cx - kx),
        f(cy + ry),
        f(cx - rx),
        f(cy + ky),
        f(cx - rx),
        f(cy),
    );
    c.cubic_to(
        f(cx - rx),
        f(cy - ky),
        f(cx - kx),
        f(cy - ry),
        f(cx),
        f(cy - ry),
    );
    c.cubic_to(
        f(cx + kx),
        f(cy - ry),
        f(cx + rx),
        f(cy - ky),
        f(cx + rx),
        f(cy),
    );
    c.close_path();
}

fn num(v: Option<&Value>, d: f64) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(d)
}

/// Entity transform: scale, rotate, then translate by origin.
fn xform(v: &Map<String, Value>) -> impl Fn((f64, f64)) -> (f64, f64) + use<> {
    let o = v.get("origin").cloned().unwrap_or(Value::Null);
    let sc = v.get("scale").cloned().unwrap_or(Value::Null);
    let (ox, oy) = (num(o.get("x"), 0.0), num(o.get("y"), 0.0));
    let nz = |x: f64| if x == 0.0 { 1.0 } else { x };
    let (sx, sy) = (
        nz(num(sc.get("width"), 1.0)),
        nz(num(sc.get("height"), 1.0)),
    );
    let r = num(v.get("rotation"), 0.0);
    let (cr, sr) = (r.cos(), r.sin());
    move |(x, y)| {
        let (x, y) = (x * sx, y * sy);
        (ox + x * cr - y * sr, oy + x * sr + y * cr)
    }
}

enum Seg {
    Move((f64, f64)),
    Line((f64, f64), (f64, f64)),
    Quad((f64, f64), (f64, f64), (f64, f64)),
    Cubic((f64, f64), (f64, f64), (f64, f64), (f64, f64)),
}

fn segments(p: &inkpath::Path, f: &dyn Fn((f64, f64)) -> (f64, f64)) -> Vec<(Seg, Option<f64>)> {
    let mut out = Vec::new();
    let mut cur: Option<(f64, f64)> = None;
    for el in &p.elements {
        let pts: Vec<(f64, f64)> = el.points.iter().map(|q| f(*q)).collect();
        let Some(&first) = pts.first() else { continue };
        if el.is_moveto() {
            cur = Some(first);
            out.push((Seg::Move(first), el.width));
            continue;
        }
        let start = cur.unwrap_or(first);
        let seg = match (el.kind, pts.as_slice()) {
            (0 | 4, [a, b, c]) => Seg::Cubic(start, *a, *b, *c),
            (1 | 5, [a, b]) => Seg::Quad(start, *a, *b),
            _ => Seg::Line(start, *pts.last().unwrap_or(&first)),
        };
        out.push((seg, el.width));
        cur = pts.last().copied();
    }
    out
}

fn emit(c: &mut Content, s: &Seg, fresh: bool) {
    let f = |v: f64| v as f32;
    let start = match s {
        Seg::Move(p) => {
            c.move_to(f(p.0), f(p.1));
            return;
        }
        Seg::Line(a, _) | Seg::Quad(a, _, _) | Seg::Cubic(a, _, _, _) => *a,
    };
    if fresh {
        c.move_to(f(start.0), f(start.1));
    }
    match s {
        Seg::Line(_, b) => {
            c.line_to(f(b.0), f(b.1));
        }
        Seg::Quad(p0, q, p1) => {
            let c1 = (
                p0.0 + (q.0 - p0.0) * 2.0 / 3.0,
                p0.1 + (q.1 - p0.1) * 2.0 / 3.0,
            );
            let c2 = (
                p1.0 + (q.0 - p1.0) * 2.0 / 3.0,
                p1.1 + (q.1 - p1.1) * 2.0 / 3.0,
            );
            c.cubic_to(f(c1.0), f(c1.1), f(c2.0), f(c2.1), f(p1.0), f(p1.1));
        }
        Seg::Cubic(_, a, b, d) => {
            c.cubic_to(f(a.0), f(a.1), f(b.0), f(b.1), f(d.0), f(d.1));
        }
        Seg::Move(_) => {}
    }
}

/// Emit a run of segments as one path (moves start subpaths; gaps start new ones).
fn emit_all(c: &mut Content, segs: &[(Seg, Option<f64>)]) {
    let mut open = false;
    for (s, _) in segs {
        emit(c, s, !open);
        open = true;
    }
}

type Stats = BTreeMap<&'static str, u64>;

fn draw_ink(
    c: &mut Content,
    ctx: &mut Ctx,
    v: &Map<String, Value>,
    extra: &[Vec<u8>],
    stats: &mut Stats,
) {
    let (r, g, b, mut a) = rgb(v.get("color"), (0.0, 0.0, 0.0));
    if v.get("tool").and_then(Value::as_u64) == Some(HIGHLIGHTER) && a >= 0.99 {
        a = 0.4;
    }
    let f = xform(v);
    let scale = {
        let s = num(v.get("scale").and_then(|s| s.get("width")), 1.0);
        if s == 0.0 { 1.0 } else { s }
    };
    let dec = |k: &str| v.get(k).and_then(bytes_of).map(|b| inkpath::decode(&b));
    let fill = match dec("encodedFillPath") {
        Some(Ok(p)) => Some(p),
        Some(Err(_)) => {
            *stats.entry("inks_skipped").or_default() += 1;
            return;
        }
        None => None,
    };
    let mut center = match dec("encodedCenterPath") {
        Some(Ok(p)) => p,
        _ => {
            *stats.entry("inks_skipped").or_default() += 1;
            return;
        }
    };
    for chunk in extra {
        match inkpath::decode(chunk) {
            Ok(p) => center.elements.extend(p.elements),
            Err(_) => {
                *stats.entry("inks_skipped").or_default() += 1;
                return;
            }
        }
    }
    c.save_state();
    if let Some(fp) = fill.filter(|p| !p.elements.is_empty()) {
        let (fr, fg, fb, fa) = if v.get("fillColor").is_some_and(Value::is_object) {
            rgb(v.get("fillColor"), (r, g, b))
        } else {
            (r, g, b, a)
        };
        ctx.gs(c, fa);
        c.set_fill_rgb(fr, fg, fb);
        emit_all(c, &segments(&fp, &f));
        c.close_path();
        c.fill_nonzero();
        c.restore_state();
        *stats.entry("inks").or_default() += 1;
        return;
    }
    let segs = segments(&center, &f);
    if segs.is_empty() {
        c.restore_state();
        return;
    }
    ctx.gs(c, a);
    c.set_stroke_rgb(r, g, b);
    c.set_fill_rgb(r, g, b);
    c.set_line_cap(LineCapStyle::RoundCap);
    c.set_line_join(LineJoinStyle::RoundJoin);
    let base = num(v.get("width"), 1.0);
    let widths: Vec<f64> = segs
        .iter()
        .filter_map(|(_, w)| *w)
        .filter(|w| *w > 0.0)
        .collect();
    let (mn, mx) = widths
        .iter()
        .fold((f64::MAX, 0.0f64), |(a, b), w| (a.min(*w), b.max(*w)));
    if widths.is_empty() || mx - mn < 0.05 * mx {
        let w = widths.first().copied().unwrap_or(base) * scale;
        c.set_line_width(w.max(0.1) as f32);
        emit_all(c, &segs);
        c.stroke();
    } else {
        for (s, w) in &segs {
            if matches!(s, Seg::Move(_)) {
                continue;
            }
            c.set_line_width((w.unwrap_or(base) * scale).max(0.1) as f32);
            emit(c, s, true);
            c.stroke();
        }
    }
    if let [(Seg::Move(p), w)] = segs.as_slice() {
        let rad = (w.unwrap_or(base) * scale / 2.0).max(0.3);
        ellipse(c, p.0 - rad, p.1 - rad, p.0 + rad, p.1 + rad);
        c.fill_nonzero();
    }
    c.restore_state();
    *stats.entry("inks").or_default() += 1;
}

fn draw_shape(c: &mut Content, ctx: &mut Ctx, v: &Map<String, Value>, stats: &mut Stats) {
    let (r, g, b, a) = rgb(v.get("color"), (0.0, 0.0, 0.0));
    let f = xform(v);
    let w = num(v.get("borderWidth"), 1.0);
    let d = v.get("definition").cloned().unwrap_or(Value::Null);
    let dv = &d["value"];
    let fill = v
        .get("fillColor")
        .filter(|x| x.is_object())
        .map(|x| rgb(Some(x), (0.0, 0.0, 0.0)))
        .filter(|x| x.3 > 0.0);
    c.save_state();
    let pt = |c: &mut Content, i: usize, p: (f64, f64)| {
        if i == 0 {
            c.move_to(p.0 as f32, p.1 as f32);
        } else {
            c.line_to(p.0 as f32, p.1 as f32);
        }
    };
    match d["type"].as_str() {
        Some("Polygon") => {
            let pts: Vec<_> = dv["points"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| f((num(p.get("x"), 0.0), num(p.get("y"), 0.0))))
                .collect();
            if pts.len() < 2 {
                c.restore_state();
                return;
            }
            for (i, p) in pts.iter().chain(pts.first()).enumerate() {
                pt(c, i, *p);
            }
        }
        Some("Line") => {
            pt(c, 0, f((0.0, 0.0)));
            pt(
                c,
                1,
                f((num(dv["end"].get("x"), 0.0), num(dv["end"].get("y"), 0.0))),
            );
        }
        Some("NormalShape") => {
            let (sw, sh) = (
                num(dv["size"].get("width"), 0.0),
                num(dv["size"].get("height"), 0.0),
            );
            let corners = [f((0.0, 0.0)), f((sw, 0.0)), f((sw, sh)), f((0.0, sh))];
            // `NormalShape.type` is an enum whose only member is Ellipse (0).
            {
                let xs = corners.iter().map(|p| p.0);
                let ys = corners.iter().map(|p| p.1);
                let (x0, x1) = (
                    xs.clone().fold(f64::MAX, f64::min),
                    xs.fold(f64::MIN, f64::max),
                );
                let (y0, y1) = (
                    ys.clone().fold(f64::MAX, f64::min),
                    ys.fold(f64::MIN, f64::max),
                );
                ellipse(c, x0, y0, x1, y1);
            }
        }
        _ => {
            c.restore_state();
            *stats.entry("shapes_skipped").or_default() += 1;
            return;
        }
    }
    ctx.gs(c, a);
    c.set_stroke_rgb(r, g, b);
    c.set_line_width(w as f32);
    if let Some((fr, fg, fb, _)) = fill {
        c.set_fill_rgb(fr, fg, fb);
        c.fill_nonzero_and_stroke();
    } else {
        c.stroke();
    }
    c.restore_state();
    *stats.entry("shapes").or_default() += 1;
}

fn draw_paper(c: &mut Content, w: f64, h: f64, bg: &Value, note_bg: &Value) {
    let mut paper = note_bg["paper"].as_object().cloned().unwrap_or_default();
    paper.extend(bg["paper"].as_object().cloned().unwrap_or_default());
    let (r, g, b, _) = rgb(paper.get("backgroundColor"), (1.0, 1.0, 1.0));
    if (r, g, b) != (1.0, 1.0, 1.0) {
        c.set_fill_rgb(r, g, b);
        c.rect(0.0, 0.0, w as f32, h as f32);
        c.fill_nonzero();
    }
    let Some(flair) = paper
        .get("flair")
        .and_then(Value::as_u64)
        .filter(|f| *f <= 2)
    else {
        return;
    };
    let sp = paper
        .get("flairSpacing")
        .and_then(Value::as_f64)
        .filter(|s| *s > 1.0)
        .unwrap_or(if flair == 0 { 24.0 } else { 20.0 });
    let mut m = note_bg["margins"].as_object().cloned().unwrap_or_default();
    m.extend(bg["margins"].as_object().cloned().unwrap_or_default());
    let mg = |k: &str| num(m.get(k), 36.0);
    let bleed = paper
        .get("flairBleeds")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let (x0, x1) = if bleed {
        (0.0, w)
    } else {
        (mg("left"), w - mg("right"))
    };
    let y0 = if bleed { sp } else { mg("top") + sp };
    let y1 = if bleed { h } else { h - mg("bottom") };
    let f = |v: f64| v as f32;
    c.save_state();
    if flair == 1 {
        c.set_fill_rgb(DOT_RGB.0, DOT_RGB.1, DOT_RGB.2);
        let mut y = y0;
        while y < y1 {
            let mut x = x0 + sp;
            while x < x1 {
                ellipse(c, x - 0.8, y - 0.8, x + 0.8, y + 0.8);
                x += sp;
            }
            y += sp;
        }
        c.fill_nonzero();
    } else {
        c.set_stroke_rgb(LINE_RGB.0, LINE_RGB.1, LINE_RGB.2);
        c.set_line_width(if flair == 0 { 0.5 } else { 0.4 });
        let mut y = y0;
        while y < y1 {
            c.move_to(f(x0), f(y));
            c.line_to(f(x1), f(y));
            y += sp;
        }
        if flair == 2 {
            let mut x = x0 + sp;
            while x < x1 {
                c.move_to(f(x), f(y0 - sp));
                c.line_to(f(x), f(y1));
                x += sp;
            }
        }
        c.stroke();
    }
    c.restore_state();
}

/// An image asset as an RGB XObject (+ alpha soft mask) written into `pdf`.
fn write_image(pdf: &mut Pdf, next: &mut Ref, data: &[u8]) -> Option<Ref> {
    let img = image::load_from_memory(data).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb_px = Vec::with_capacity((w * h * 3) as usize);
    let mut alpha = Vec::with_capacity((w * h) as usize);
    for p in rgba.pixels() {
        rgb_px.extend_from_slice(&p.0[..3]);
        alpha.push(p.0[3]);
    }
    let deflate = |b: &[u8]| {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).ok()?;
        e.finish().ok()
    };
    let id = next.bump();
    let mask = alpha.iter().any(|a| *a < 255).then(|| next.bump());
    let px = deflate(&rgb_px)?;
    let mut x = pdf.image_xobject(id, &px);
    x.filter(Filter::FlateDecode);
    x.width(w as i32).height(h as i32).bits_per_component(8);
    x.color_space().device_rgb();
    if let Some(m) = mask {
        x.s_mask(m);
    }
    x.finish();
    if let Some(m) = mask {
        let ap = deflate(&alpha)?;
        let mut s = pdf.image_xobject(m, &ap);
        s.filter(Filter::FlateDecode);
        s.width(w as i32).height(h as i32).bits_per_component(8);
        s.color_space().device_gray();
        s.finish();
    }
    Some(id)
}

/// Render the note to a PDF. `assets` maps sha512 -> bytes (see [`needed_assets`]); missing ones
/// fall back to paper / a dashed placeholder. Returns the PDF and counts of what was drawn.
pub fn render(
    st: &NoteState,
    assets: &HashMap<String, Vec<u8>>,
    label: bool,
) -> Result<(Vec<u8>, Value)> {
    let mut stats: Stats = BTreeMap::new();
    let note_bg = st.note_background();
    let pdf_index = pdf_page_index(st);
    let ents = entities(st);
    let page_no: HashMap<SeqId, usize> = st.pages().into_iter().map(|p| (p.id, p.index)).collect();
    let mut by_page: HashMap<usize, Vec<(i64, Id)>> = HashMap::new();
    for (id, e) in &ents {
        if let Some(n) =
            e.v.get("page")
                .and_then(seq_of)
                .and_then(|p| page_no.get(&p))
        {
            let z = e.v.get("zIndex").and_then(Value::as_i64).unwrap_or(0);
            by_page.entry(*n).or_default().push((z, *id));
        }
    }

    let mut pdf = Pdf::new();
    let mut next = Ref::new(1);
    let catalog = next.bump();
    let tree = next.bump();
    let (f1, f2) = (next.bump(), next.bump());
    let mut ctx = Ctx {
        alphas: Vec::new(),
        images: Vec::new(),
    };
    let mut image_of: HashMap<String, Option<usize>> = HashMap::new();
    // source PDFs: parse once, extract each used page as an XObject.
    let mut sources: HashMap<String, Option<hayro_write::hayro_syntax::Pdf>> = HashMap::new();
    let mut src_pages: Vec<(String, usize)> = Vec::new();

    let mut pages = st.pages();
    if pages.is_empty() {
        pages.push(nb_domain::state::Page {
            index: 1,
            id: (0, 0, 0),
            node: (0, 0, 0),
            background: None,
        });
    }
    let mut outs: Vec<PageOut> = Vec::new();
    for pg in &pages {
        let bg = pg.background.clone().unwrap_or_else(|| note_bg.clone());
        let (mut w, mut h) = st.page_size(pg);
        let pdfv = &bg["pdf"];
        let mut src = None;
        if pdfv.is_object()
            && let Some(sha) = asset_hash(pdfv)
        {
            {
                let doc = sources.entry(sha.clone()).or_insert_with(|| {
                    let d = assets.get(&sha).and_then(|b| open(b).ok());
                    if d.is_none() && !assets.contains_key(&sha) {
                        *stats.entry("assets_missing").or_default() += 1;
                    }
                    d
                });
                let idx = pdf_index.get(&pg.id).copied().unwrap_or_else(|| {
                    pdfv["pageOffset"].as_u64().unwrap_or(0) as usize + pg.id.2 as usize
                });
                if let Some(d) = doc
                    && let Some(p) = d.pages().get(idx)
                {
                    let (pw, ph) = p.render_dimensions();
                    (w, h) = (f64::from(pw), f64::from(ph));
                    let k = src_pages
                        .iter()
                        .position(|s| *s == (sha.clone(), idx))
                        .unwrap_or_else(|| {
                            src_pages.push((sha.clone(), idx));
                            src_pages.len() - 1
                        });
                    src = Some(k);
                    *stats.entry("pdf_pages").or_default() += 1;
                }
            }
        }
        let mut c = Content::new();
        if src.is_some() {
            c.save_state();
            c.x_object(Name(b"Src"));
            c.restore_state();
        }
        c.save_state();
        c.transform([1.0, 0.0, 0.0, -1.0, 0.0, h as f32]);
        if src.is_none() {
            if pdfv.is_object() {
                *stats.entry("pdf_pages_unavailable").or_default() += 1;
            }
            draw_paper(&mut c, w, h, &bg, &note_bg);
            *stats.entry("paper_pages").or_default() += 1;
        }
        let mut items = by_page.remove(&pg.index).unwrap_or_default();
        items.sort();
        let is_hl = |e: &Entity| {
            e.kind == "ink" && e.v.get("tool").and_then(Value::as_u64) == Some(HIGHLIGHTER)
        };
        for (_, id) in &items {
            let e = &ents[id];
            if e.kind != "block" {
                continue;
            }
            draw_block(
                &mut c,
                &mut ctx,
                st,
                *id,
                &e.v,
                assets,
                &mut pdf,
                &mut next,
                &mut image_of,
                &mut stats,
            );
        }
        for (_, id) in &items {
            let e = &ents[id];
            if is_hl(e) {
                draw_ink(&mut c, &mut ctx, &e.v, &e.extra, &mut stats);
            }
        }
        for (_, id) in &items {
            let e = &ents[id];
            match e.kind {
                "shape" => draw_shape(&mut c, &mut ctx, &e.v, &mut stats),
                "ink" if !is_hl(e) => draw_ink(&mut c, &mut ctx, &e.v, &e.extra, &mut stats),
                _ => {}
            }
        }
        outs.push(PageOut { w, h, src, c });
    }

    // typed body text, flowed from page 1 (pages appended on overflow).
    let body = st.text(None);
    if !body.trim().is_empty() {
        let fs = st
            .metadata()
            .get("defaultFontSize")
            .and_then(Value::as_f64)
            .unwrap_or(14.0);
        let m = &note_bg["margins"];
        let mg = |k: &str| num(m.get(k), 36.0);
        let mut lines: Vec<String> = body.split('\n').map(str::to_string).collect();
        lines.reverse();
        let mut n = 0u64;
        let mut i = 0;
        while !lines.is_empty() {
            if i >= outs.len() {
                let (w, h) = outs.last().map_or((612.0, 792.0), |p| (p.w, p.h));
                let mut c = Content::new();
                c.save_state();
                c.transform([1.0, 0.0, 0.0, -1.0, 0.0, h as f32]);
                outs.push(PageOut { w, h, src: None, c });
            }
            let p = &mut outs[i];
            let width = p.w - mg("left") - mg("right");
            let mut y = mg("top") + fs;
            let bottom = p.h - mg("bottom");
            while y <= bottom {
                let Some(ln) = lines.pop() else { break };
                let mut wrapped = if ln.is_empty() {
                    vec![String::new()]
                } else {
                    wrap(&ln, Font::Helv, fs, width)
                };
                let first = wrapped.remove(0);
                for rest in wrapped.into_iter().rev() {
                    lines.push(rest);
                }
                if !first.is_empty() {
                    text_at(
                        &mut p.c,
                        Font::Helv,
                        fs,
                        mg("left"),
                        y,
                        &first,
                        (0.0, 0.0, 0.0),
                    );
                }
                y += fs * 1.4;
                n += 1;
            }
            i += 1;
        }
        stats.insert("body_lines", n);
    }
    if label {
        for p in &mut outs {
            text_at(
                &mut p.c,
                Font::Helv,
                5.5,
                6.0,
                p.h - 4.0,
                LABEL,
                (0.55, 0.55, 0.55),
            );
        }
    }

    // extract the used source pages as XObjects.
    let mut src_refs: Vec<Option<Ref>> = vec![None; src_pages.len()];
    let mut shas: Vec<&String> = src_pages.iter().map(|(s, _)| s).collect();
    shas.dedup();
    let mut seen = Vec::new();
    for sha in shas {
        if seen.contains(&sha) {
            continue;
        }
        seen.push(sha);
        let Some(Some(doc)) = sources.get(sha) else {
            continue;
        };
        let want: Vec<(usize, usize)> = src_pages
            .iter()
            .enumerate()
            .filter(|(_, (s, _))| s == sha)
            .map(|(k, (_, i))| (k, *i))
            .collect();
        let queries: Vec<_> = want
            .iter()
            .map(|(_, i)| ExtractionQuery::new_xobject(*i))
            .collect();
        let ex = extract(
            doc,
            Box::new(|| next.bump()),
            ChunkSettings::default(),
            &queries,
        )
        .map_err(|e| PdfError(format!("source pdf: {e:?}")))?;
        for ((k, _), r) in want.iter().zip(&ex.root_refs) {
            src_refs[*k] = Some(r.map_err(|e| PdfError(format!("source pdf: {e:?}")))?);
        }
        pdf.extend(&ex.chunk);
    }

    let gs_refs: Vec<Ref> = ctx.alphas.iter().map(|_| next.bump()).collect();
    for (r, a) in gs_refs.iter().zip(&ctx.alphas) {
        pdf.ext_graphics(*r)
            .non_stroking_alpha(*a)
            .stroking_alpha(*a);
    }
    pdf.type1_font(f1)
        .base_font(Name(b"Helvetica"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.type1_font(f2)
        .base_font(Name(b"Courier"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    let mut kids = Vec::new();
    for p in outs {
        let (pid, cid) = (next.bump(), next.bump());
        kids.push(pid);
        let mut c = p.c;
        c.restore_state();
        let bytes = c.finish();
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(bytes.as_slice())
            .map_err(|e| PdfError(e.to_string()))?;
        let z = e.finish().map_err(|e| PdfError(e.to_string()))?;
        pdf.stream(cid, &z).filter(Filter::FlateDecode);
        let mut page = pdf.page(pid);
        page.media_box(Rect::new(0.0, 0.0, p.w as f32, p.h as f32))
            .parent(tree)
            .contents(cid);
        let mut res = page.resources();
        {
            let mut fonts = res.fonts();
            fonts.pair(Name(b"F1"), f1);
            fonts.pair(Name(b"F2"), f2);
        }
        if !gs_refs.is_empty() {
            let mut g = res.ext_g_states();
            for (i, r) in gs_refs.iter().enumerate() {
                g.pair(Name(format!("G{i}").as_bytes()), *r);
            }
        }
        let mut xo = res.x_objects();
        if let Some(r) = p.src.and_then(|k| src_refs[k]) {
            xo.pair(Name(b"Src"), r);
        }
        for (i, r) in ctx.images.iter().enumerate() {
            xo.pair(Name(format!("Im{i}").as_bytes()), *r);
        }
    }
    let n = kids.len();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids(kids.iter().copied()).count(n as i32);
    let info = next.bump();
    let title = st.title().unwrap_or_default();
    pdf.document_info(info)
        .title(TextStr(&title))
        .producer(TextStr("notability-cli local renderer"))
        .creator(TextStr("notability-cli"))
        .subject(TextStr(LABEL));
    let bytes = pdf.finish();
    let mut out: Map<String, Value> = Map::new();
    out.insert("pages".into(), json!(n));
    out.insert("bytes".into(), json!(bytes.len()));
    out.insert("renderer".into(), json!("local"));
    for (k, v) in stats {
        out.insert(k.into(), json!(v));
    }
    Ok((bytes, Value::Object(out)))
}

#[allow(clippy::too_many_arguments)]
fn draw_block(
    c: &mut Content,
    ctx: &mut Ctx,
    st: &NoteState,
    id: Id,
    v: &Map<String, Value>,
    assets: &HashMap<String, Vec<u8>>,
    pdf: &mut Pdf,
    next: &mut Ref,
    image_of: &mut HashMap<String, Option<usize>>,
    stats: &mut Stats,
) {
    let o = v.get("origin").cloned().unwrap_or(Value::Null);
    let sz = v.get("size").cloned().unwrap_or(Value::Null);
    let sc = v.get("scale").cloned().unwrap_or(Value::Null);
    let nz = |x: f64| if x == 0.0 { 1.0 } else { x };
    let (x, y) = (num(o.get("x"), 0.0), num(o.get("y"), 0.0));
    let w = num(sz.get("width"), 100.0) * nz(num(sc.get("width"), 1.0));
    let h = num(sz.get("height"), 30.0) * nz(num(sc.get("height"), 1.0));
    if let Some(img) = v.get("image").filter(|i| i.is_object()) {
        let idx = asset_hash(img).and_then(|sha| {
            *image_of.entry(sha.clone()).or_insert_with(|| {
                let r = assets.get(&sha).and_then(|d| write_image(pdf, next, d))?;
                ctx.images.push(r);
                Some(ctx.images.len() - 1)
            })
        });
        c.save_state();
        if let Some(i) = idx {
            let fh = v.get("imageFlippedHorizontally") == Some(&Value::Bool(true));
            let fv = v.get("imageFlippedVertically") == Some(&Value::Bool(true));
            let (a, e) = if fh { (-w, x + w) } else { (w, x) };
            let (d, f) = if fv { (h, y) } else { (-h, y + h) };
            c.transform([a as f32, 0.0, 0.0, d as f32, e as f32, f as f32]);
            c.x_object(Name(format!("Im{i}").as_bytes()));
            *stats.entry("images").or_default() += 1;
        } else {
            c.set_stroke_rgb(0.7, 0.7, 0.7);
            c.set_line_width(0.5);
            c.set_dash_pattern([2.0], 0.0);
            c.rect(x as f32, y as f32, w as f32, h as f32);
            c.stroke();
            *stats.entry("images_missing").or_default() += 1;
        }
        c.restore_state();
        return;
    }
    if let Some(tex) = v
        .get("mathLatex")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let (r, g, b, _) = rgb(v.get("mathColor"), (0.0, 0.0, 0.0));
        let mut ly = y + 11.0;
        for para in tex.split('\n') {
            for ln in wrap(para, Font::Cour, 11.0, w.max(40.0)) {
                text_at(c, Font::Cour, 11.0, x, ly, &ln, (r, g, b));
                ly += 11.0 * 1.2;
            }
        }
        *stats.entry("math").or_default() += 1;
        return;
    }
    let text = st.text(Some(id));
    if !text.is_empty() {
        let m = v.get("margins").cloned().unwrap_or(Value::Null);
        let (l, t, r) = (
            num(m.get("left"), 5.0),
            num(m.get("top"), 3.0),
            num(m.get("right"), 5.0),
        );
        let width = (w - l - r).max(40.0);
        let mut ly = y + t + 12.0;
        for para in text.split('\n') {
            for ln in wrap(para, Font::Helv, 12.0, width) {
                if !ln.is_empty() {
                    text_at(c, Font::Helv, 12.0, x + l, ly, &ln, (0.0, 0.0, 0.0));
                }
                ly += 12.0 * 1.2;
            }
        }
        *stats.entry("text_boxes").or_default() += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_by_helvetica_width() {
        let lines = wrap(
            "the quick brown fox jumps over the lazy dog",
            Font::Helv,
            10.0,
            60.0,
        );
        assert!(lines.len() > 2);
        assert!(
            lines
                .iter()
                .all(|l| Font::Helv.width(l, 10.0) <= 60.0 || !l.contains(' '))
        );
    }

    #[test]
    fn winansi_maps_quotes() {
        assert_eq!(winansi("a\u{2019}b\u{4e00}"), vec![b'a', 0x92, b'b', b'?']);
    }

    #[test]
    fn renders_text_only_note() {
        let bundle = json!({"operations": []});
        let st = NoteState::new(&bundle);
        let (pdf, stats) = render(&st, &HashMap::new(), true).unwrap();
        assert_eq!(stats["pages"], 1);
        assert_eq!(crate::info::page_sizes(&pdf).unwrap().len(), 1);
        crate::raster::page_png(&pdf, 0, 0.5).unwrap();
    }
}
