//! Fast reader for a note's operation log (collab `POST /note/{id}/bundle`). Reads only the
//! fields text reconstruction needs, straight from the FlatBuffer, and tolerates partial
//! vtables. Typed text is an RGA sequence: each inserted character is identified by
//! (site, timestamp, index) and placed after its anchor; concurrent siblings newest first.
//! Ink is vector data and is not converted to text here.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::schema::UNIONS;

pub type OpId = (u16, u32);
pub type CharId = (u16, u32, u32);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct BundleFormatError(String);

fn oob() -> BundleFormatError {
    BundleFormatError("bundle offset out of range".into())
}

#[derive(Clone, Copy)]
struct Tab<'a> {
    b: &'a [u8],
    pos: usize,
    vt: usize,
    vlen: usize,
}

fn rd<const N: usize>(b: &[u8], p: usize) -> Result<[u8; N], BundleFormatError> {
    b.get(p..p + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(oob)
}
fn u16_at(b: &[u8], p: usize) -> Result<u16, BundleFormatError> {
    rd::<2>(b, p).map(u16::from_le_bytes)
}
fn u32_at(b: &[u8], p: usize) -> Result<u32, BundleFormatError> {
    rd::<4>(b, p).map(u32::from_le_bytes)
}

impl<'a> Tab<'a> {
    fn new(b: &'a [u8], pos: usize) -> Result<Self, BundleFormatError> {
        let rel = i64::from(i32::from_le_bytes(rd::<4>(b, pos)?));
        let vt = usize::try_from(pos as i64 - rel).map_err(|_| oob())?;
        let vlen = usize::from(u16_at(b, vt)?);
        Ok(Self { b, pos, vt, vlen })
    }
    fn off(&self, o: usize) -> Result<usize, BundleFormatError> {
        if o < self.vlen {
            u16_at(self.b, self.vt + o).map(usize::from)
        } else {
            Ok(0)
        }
    }
    fn at(&self, o: usize) -> Result<Option<usize>, BundleFormatError> {
        Ok(match self.off(o)? {
            0 => None,
            x => Some(self.pos + x),
        })
    }
    fn indirect(&self, o: usize) -> Result<Option<usize>, BundleFormatError> {
        match self.at(o)? {
            None => Ok(None),
            Some(p) => Ok(Some(p + u32_at(self.b, p)? as usize)),
        }
    }
    fn table(&self, o: usize) -> Result<Option<Tab<'a>>, BundleFormatError> {
        self.indirect(o)?.map(|p| Tab::new(self.b, p)).transpose()
    }
    fn string(&self, o: usize) -> Result<Option<String>, BundleFormatError> {
        let Some(p) = self.indirect(o)? else {
            return Ok(None);
        };
        let n = u32_at(self.b, p)? as usize;
        let s = self.b.get(p + 4..p + 4 + n).ok_or_else(oob)?;
        Ok(Some(String::from_utf8_lossy(s).into_owned()))
    }
    fn vector(&self, o: usize) -> Result<(usize, usize), BundleFormatError> {
        match self.indirect(o)? {
            None => Ok((0, 0)),
            Some(p) => Ok((p + 4, u32_at(self.b, p)? as usize)),
        }
    }
    fn u8(&self, o: usize) -> Result<u8, BundleFormatError> {
        self.at(o)?
            .map_or(Ok(0), |p| self.b.get(p).copied().ok_or_else(oob))
    }
    fn u16(&self, o: usize) -> Result<u16, BundleFormatError> {
        self.at(o)?.map_or(Ok(0), |p| u16_at(self.b, p))
    }
    fn u32(&self, o: usize) -> Result<u32, BundleFormatError> {
        self.at(o)?.map_or(Ok(0), |p| u32_at(self.b, p))
    }
    fn u64(&self, o: usize) -> Result<u64, BundleFormatError> {
        self.at(o)?
            .map_or(Ok(0), |p| rd::<8>(self.b, p).map(u64::from_le_bytes))
    }
    fn id(&self, o: usize) -> Result<Option<OpId>, BundleFormatError> {
        self.at(o)?
            .map(|p| Ok((u16_at(self.b, p)?, u32_at(self.b, p + 4)?)))
            .transpose()
    }
    fn seq(&self, o: usize) -> Result<Option<CharId>, BundleFormatError> {
        self.at(o)?.map(|p| seq_at(self.b, p)).transpose()
    }
}

fn seq_at(b: &[u8], p: usize) -> Result<CharId, BundleFormatError> {
    Ok((u16_at(b, p)?, u32_at(b, p + 4)?, u32_at(b, p + 8)?))
}

/// Payload kind name for a wire tag (`NONE` for 0, `Unknown<tag>` past the schema).
pub fn payload_name(tag: u8) -> String {
    if tag == 0 {
        return "NONE".into();
    }
    let u = UNIONS
        .iter()
        .find(|u| u.name == "PayloadUnion")
        .expect("PayloadUnion in schema");
    u.variants
        .get(usize::from(tag) - 1)
        .map_or_else(|| format!("Unknown{tag}"), |(name, _)| (*name).to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub op_id: OpId,
    pub anchor: Option<CharId>,
    pub text: Vec<char>,
    pub field: Option<OpId>,
}

#[derive(Debug, Default)]
pub struct DecodedNote {
    pub note_id: Option<String>,
    pub creator_user_id: Option<String>,
    pub created_at: u64,
    pub schema_version: u16,
    pub op_count: usize,
    /// Op kind -> count; unknown wire tags appear as `Unknown<tag>`.
    pub op_types: BTreeMap<String, usize>,
    pub runs: Vec<TextRun>,
    pub removed: HashSet<CharId>,
    pub revived: Vec<CharId>,
    pub asset_hashes: Vec<String>,
    pub recording_count: usize,
    pub page_ops: usize,
    pub ink_strokes: usize,
}

impl DecodedNote {
    /// Visible text of every text field; key `None` is the note body, others are text boxes
    /// keyed by their creating op id.
    pub fn text_fields(&self) -> HashMap<Option<OpId>, String> {
        let mut by_field: HashMap<Option<OpId>, Vec<&TextRun>> = HashMap::new();
        for r in &self.runs {
            by_field.entry(r.field).or_default().push(r);
        }
        let mut removed = self.removed.clone();
        for loc in &self.revived {
            removed.remove(loc);
        }
        by_field
            .into_iter()
            .map(|(f, runs)| (f, linearize(&runs, &removed)))
            .collect()
    }

    /// The note body's text.
    pub fn text(&self) -> String {
        self.text_fields().remove(&None).unwrap_or_default()
    }
}

/// Depth-first RGA walk. Anything anchored directly after a character is newer than the run's
/// own continuation, so it is emitted first.
pub fn linearize(runs: &[&TextRun], removed: &HashSet<CharId>) -> String {
    let mut children: HashMap<Option<CharId>, Vec<&TextRun>> = HashMap::new();
    for r in runs {
        children.entry(r.anchor).or_default().push(r);
    }
    for v in children.values_mut() {
        // newest first: (timestamp, site) descending; stable
        v.sort_by(|a, b| (b.op_id.1, b.op_id.0).cmp(&(a.op_id.1, a.op_id.0)));
    }
    let mut out = String::new();
    let mut st: Vec<(&TextRun, usize)> = Vec::new();
    fn push<'a>(
        children: &HashMap<Option<CharId>, Vec<&'a TextRun>>,
        anchor: Option<CharId>,
        st: &mut Vec<(&'a TextRun, usize)>,
    ) {
        if let Some(kids) = children.get(&anchor) {
            st.extend(kids.iter().rev().map(|r| (*r, 0)));
        }
    }
    push(&children, None, &mut st);
    while let Some(top) = st.last_mut() {
        let (r, i) = *top;
        if i >= r.text.len() {
            st.pop();
            continue;
        }
        let (site, ts) = r.op_id;
        let id = (site, ts, i as u32);
        if !removed.contains(&id) {
            out.push(r.text[i]);
        }
        top.1 = i + 1;
        push(&children, Some(id), &mut st);
    }
    out
}

/// Decode a `NoteBundle` into a [`DecodedNote`].
pub fn decode_bundle(data: &[u8]) -> Result<DecodedNote, BundleFormatError> {
    if data.len() < 8 {
        return Err(BundleFormatError("bundle too small".into()));
    }
    let b = data;
    let root = Tab::new(b, u32_at(b, 0)? as usize)?;
    let (vec, n) = root.vector(16)?;
    let note_id = match root.at(4)? {
        Some(p) => {
            let lo = u64::from_le_bytes(rd::<8>(b, p)?);
            let hi = u64::from_le_bytes(rd::<8>(b, p + 8)?);
            Some(crate::codec::uuid_string(
                (u128::from(hi) << 64) | u128::from(lo),
            ))
        }
        None => None,
    };
    let mut dn = DecodedNote {
        note_id,
        creator_user_id: root.string(14)?,
        created_at: root.u64(12)?,
        schema_version: root.u16(18)?,
        op_count: n,
        ..Default::default()
    };
    let mut seen_assets = HashSet::new();
    for i in 0..n {
        let q = vec + 4 * i;
        let op = Tab::new(b, q + u32_at(b, q)? as usize)?;
        let name = payload_name(op.u8(12)?);
        match name.as_str() {
            "CreateRecording" => dn.recording_count += 1,
            "CreatePage" => dn.page_ops += 1,
            "CreateInk" => dn.ink_strokes += 1,
            _ => {}
        }
        let (Some(oid), Some(pl)) = (op.id(4)?, op.table(14)?) else {
            *dn.op_types.entry(name).or_default() += 1;
            continue;
        };
        match name.as_str() {
            "InsertString" => dn.runs.push(TextRun {
                op_id: oid,
                anchor: pl.seq(4)?,
                text: pl.string(6)?.unwrap_or_default().chars().collect(),
                field: pl.id(8)?,
            }),
            "InsertChar" => dn.runs.push(TextRun {
                op_id: oid,
                anchor: pl.seq(4)?,
                text: vec![char::from_u32(pl.u32(6)?).unwrap_or('\u{fffd}')],
                field: pl.id(8)?,
            }),
            "RemoveChar" => {
                if let Some(l) = pl.seq(4)? {
                    dn.removed.insert(l);
                }
            }
            "RemoveChars" | "ReviveChars" => {
                let (v, m) = pl.vector(4)?;
                for k in 0..m {
                    let l = seq_at(b, v + 12 * k)?;
                    if name == "RemoveChars" {
                        dn.removed.insert(l);
                    } else {
                        dn.revived.push(l);
                    }
                }
            }
            "AssetCloudPersisted" => {
                if let Some(p) = pl.at(4)? {
                    let hex = crate::codec::hex(b.get(p..p + 64).ok_or_else(oob)?);
                    if seen_assets.insert(hex.clone()) {
                        dn.asset_hashes.push(hex);
                    }
                }
            }
            _ => {}
        }
        *dn.op_types.entry(name).or_default() += 1;
    }
    Ok(dn)
}

/// File extension from magic bytes.
pub fn sniff_extension(head: &[u8]) -> &'static str {
    let at = |r: std::ops::Range<usize>| head.get(r).unwrap_or_default();
    if head.starts_with(b"%PDF") {
        "pdf"
    } else if head.starts_with(b"\x89PNG") {
        "png"
    } else if head.starts_with(b"\xff\xd8\xff") {
        "jpg"
    } else if head.starts_with(b"RIFF") && at(8..12) == b"WEBP" {
        "webp"
    } else if at(4..8) == b"ftyp" {
        "m4a"
    } else if head.starts_with(b"OggS") {
        "ogg"
    } else if head.starts_with(b"ID3") || head.starts_with(b"\xff\xfb") {
        "mp3"
    } else if head.starts_with(b"GIF8") {
        "gif"
    } else if head.starts_with(b"PK\x03\x04") {
        "zip"
    } else {
        "bin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_bundle_text() {
        let raw = include_bytes!("../tests/fixtures/synthetic_bundle.bin");
        let dn = decode_bundle(raw).unwrap();
        assert_eq!(dn.op_count, 10);
        assert_eq!(dn.text(), "Hello world!");
        let fields = dn.text_fields();
        assert_eq!(fields.get(&Some((2, 55))).map(String::as_str), Some("box"));
        assert_eq!(dn.asset_hashes.len(), 1);
        assert_eq!(
            dn.asset_hashes[0],
            (0u8..64).map(|b| format!("{b:02x}")).collect::<String>()
        );
        assert_eq!(dn.page_ops, 1);
        assert_eq!(dn.ink_strokes, 1);
        assert_eq!(dn.schema_version, 3);
        assert_eq!(
            dn.creator_user_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
    }

    #[test]
    fn unknown_tag_is_counted() {
        use serde_json::json;
        let bundle = |payload: &str| {
            let v = json!({"ops": [{"id": {"site": 1, "timestamp": 2}, "payload": {"type": payload, "value": {}}}]});
            crate::codec::encode("NoteBundle", &v).unwrap()
        };
        // CreatePage (tag 3) and ModifyPage (tag 4) differ only in the tag byte.
        let (a, b) = (bundle("CreatePage"), bundle("ModifyPage"));
        let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        assert_eq!(diff.len(), 1);
        let mut patched = a.clone();
        patched[diff[0]] = 200;
        let dn = decode_bundle(&patched).unwrap();
        assert_eq!(dn.op_types.get("Unknown200"), Some(&1));
        assert_eq!(decode_bundle(&a).unwrap().page_ops, 1);
    }

    #[test]
    fn sniff() {
        assert_eq!(sniff_extension(b"%PDF-1.7"), "pdf");
        assert_eq!(sniff_extension(b"\0\0\0\x18ftypM4A "), "m4a");
        assert_eq!(sniff_extension(b"??"), "bin");
    }
}
