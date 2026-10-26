//! Schema-driven FlatBuffers codec over [`serde_json::Value`].
//!
//! Decoding yields stable JSON shapes (structs as objects, union `x` as `{"type", "value"}`, `AssetHash` as hex, `Uuid` as a
//! canonical string, `[ubyte]` as an array of integers) and encoding produces deterministic bytes
//! (forward layout: vtable, then table, then children in field order).
//!
//! One deliberate difference: a union tag the schema does not know decodes to
//! `{"type": "Unknown", "tag": n, "raw": "<hex of the inline table>"}` instead of failing, so
//! notes written by newer clients still open. Such values cannot be re-encoded.

use serde_json::{Map, Value};

use crate::schema::{El, STRUCTS, Sc, TABLES, Ty, UNIONS, Variant};

/// Name used for union values whose wire tag is not in the schema.
pub const UNKNOWN: &str = "Unknown";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("malformed {table}: {detail}")]
    Malformed { table: String, detail: String },
    #[error("unknown table {0}")]
    UnknownTable(String),
    #[error("{0}")]
    Invalid(String),
}

fn invalid(msg: impl Into<String>) -> CodecError {
    CodecError::Invalid(msg.into())
}

// ------------------------------------------------------------------------------------ decode

struct Reader<'a> {
    b: &'a [u8],
}

impl Reader<'_> {
    fn bytes<const N: usize>(&self, pos: usize) -> Result<[u8; N], String> {
        pos.checked_add(N)
            .and_then(|end| self.b.get(pos..end))
            .map(|s| s.try_into().expect("slice length"))
            .ok_or_else(|| {
                format!(
                    "read of {N} bytes at {pos} beyond buffer of {}",
                    self.b.len()
                )
            })
    }
    fn u16(&self, pos: usize) -> Result<u16, String> {
        self.bytes::<2>(pos).map(u16::from_le_bytes)
    }
    fn u32(&self, pos: usize) -> Result<u32, String> {
        self.bytes::<4>(pos).map(u32::from_le_bytes)
    }
    fn i32(&self, pos: usize) -> Result<i32, String> {
        self.bytes::<4>(pos).map(i32::from_le_bytes)
    }
    fn u64(&self, pos: usize) -> Result<u64, String> {
        self.bytes::<8>(pos).map(u64::from_le_bytes)
    }
    fn follow(&self, pos: usize) -> Result<usize, String> {
        Ok(pos + self.u32(pos)? as usize)
    }

    fn scalar(&self, sc: Sc, pos: usize) -> Result<Value, String> {
        Ok(match sc {
            Sc::Bool => Value::Bool(self.bytes::<1>(pos)?[0] != 0),
            Sc::U8 => self.bytes::<1>(pos)?[0].into(),
            Sc::I8 => (self.bytes::<1>(pos)?[0] as i8).into(),
            Sc::U16 => self.u16(pos)?.into(),
            Sc::I16 => i16::from_le_bytes(self.bytes(pos)?).into(),
            Sc::U32 => self.u32(pos)?.into(),
            Sc::I32 => self.i32(pos)?.into(),
            Sc::U64 => self.u64(pos)?.into(),
            Sc::I64 => i64::from_le_bytes(self.bytes(pos)?).into(),
            Sc::F32 => f64::from(f32::from_le_bytes(self.bytes(pos)?)).into(),
            Sc::F64 => f64::from_le_bytes(self.bytes(pos)?).into(),
        })
    }

    fn strukt(&self, pos: usize, si: usize) -> Result<Value, String> {
        let d = &STRUCTS[si];
        match d.name {
            "AssetHash" => {
                let raw = pos
                    .checked_add(64)
                    .and_then(|e| self.b.get(pos..e))
                    .ok_or("AssetHash beyond buffer")?;
                return Ok(hex(raw).into());
            }
            "Uuid" => {
                let lo = self.u64(pos)?;
                let hi = self.u64(pos + 8)?;
                return Ok(uuid_string((u128::from(hi) << 64) | u128::from(lo)).into());
            }
            _ => {}
        }
        let mut out = Map::new();
        for f in d.fields {
            let v = match f.ty {
                Ty::Struct(s) => self.strukt(pos + f.offset, s)?,
                Ty::Scalar(sc) => self.scalar(sc, pos + f.offset)?,
                other => return Err(format!("struct field of type {other:?}")),
            };
            out.insert(f.name.into(), v);
        }
        Ok(Value::Object(out))
    }

    fn string(&self, pos: usize) -> Result<String, String> {
        let n = self.u32(pos)? as usize;
        let s = (pos + 4)
            .checked_add(n)
            .and_then(|e| self.b.get(pos + 4..e))
            .ok_or("string beyond buffer")?;
        Ok(String::from_utf8_lossy(s).into_owned())
    }

    fn table(&self, tpos: usize, ti: usize, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err("tables nested too deeply".into());
        }
        let vt = usize::try_from(tpos as i64 - i64::from(self.i32(tpos)?))
            .map_err(|_| "vtable offset out of range".to_string())?;
        let vtsize = self.u16(vt)? as usize;
        let mut out = Map::new();
        let mut unions = Vec::new();
        for f in TABLES[ti].fields {
            let vo = 4 + 2 * f.slot;
            let off = if vo < vtsize {
                self.u16(vt + vo)? as usize
            } else {
                0
            };
            if off == 0 {
                continue;
            }
            let p = tpos + off;
            let v = match f.ty {
                Ty::Union(u) => {
                    unions.push((f.name, u, p));
                    continue;
                }
                Ty::Scalar(sc) => self.scalar(sc, p)?,
                Ty::Struct(s) => self.strukt(p, s)?,
                Ty::String => self.string(self.follow(p)?)?.into(),
                Ty::Table(t) => self.table(self.follow(p)?, t, depth + 1)?,
                Ty::Vector(el) => self.vector(self.follow(p)?, el, depth)?,
            };
            out.insert(f.name.into(), v);
        }
        for (name, ui, p) in unions {
            let tag = match out.remove(&format!("{name}Type")) {
                Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
                _ => 0,
            };
            if tag == 0 {
                continue;
            }
            let q = self.follow(p)?;
            let value = match UNIONS[ui].variants.get(tag - 1) {
                Some((vname, Variant::Table(t))) => {
                    serde_json::json!({"type": vname, "value": self.table(q, *t, depth + 1)?})
                }
                Some((vname, Variant::Struct(s))) => {
                    serde_json::json!({"type": vname, "value": self.strukt(q, *s)?})
                }
                None => serde_json::json!({"type": UNKNOWN, "tag": tag, "raw": self.inline_hex(q)}),
            };
            out.insert(name.into(), value);
        }
        Ok(Value::Object(out))
    }

    /// Hex of a table's inline bytes (soffset + fields), for unknown union members.
    fn inline_hex(&self, tpos: usize) -> String {
        let size = self
            .i32(tpos)
            .ok()
            .and_then(|so| usize::try_from(tpos as i64 - i64::from(so)).ok())
            .and_then(|vt| self.u16(vt + 2).ok())
            .map_or(0, usize::from);
        hex(self
            .b
            .get(tpos..tpos.saturating_add(size))
            .unwrap_or_default())
    }

    fn vector(&self, q: usize, el: El, depth: usize) -> Result<Value, String> {
        let n = self.u32(q)? as usize;
        let e0 = q + 4;
        if n > self.b.len() {
            return Err(format!("vector length {n} exceeds buffer"));
        }
        let mut items = Vec::with_capacity(n);
        for i in 0..n {
            items.push(match el {
                El::Struct(s) => self.strukt(e0 + i * STRUCTS[s].size, s)?,
                El::Table(t) => self.table(self.follow(e0 + 4 * i)?, t, depth + 1)?,
                El::String => self.string(self.follow(e0 + 4 * i)?)?.into(),
                El::Scalar(sc) => self.scalar(sc, e0 + i * sc.size())?,
            });
        }
        Ok(Value::Array(items))
    }
}

/// Decode a FlatBuffer whose root is `table`.
pub fn decode(table: &str, data: &[u8]) -> Result<Value, CodecError> {
    let ti = crate::schema::table(table).ok_or_else(|| CodecError::UnknownTable(table.into()))?;
    let r = Reader { b: data };
    r.u32(0)
        .and_then(|root| r.table(root as usize, ti, 0))
        .map_err(|detail| CodecError::Malformed {
            table: table.into(),
            detail,
        })
}

// ------------------------------------------------------------------------------------ encode

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn int_of(v: &Value) -> Result<i128, CodecError> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Number(n) => n
            .as_u64()
            .map(i128::from)
            .or_else(|| n.as_i64().map(i128::from))
            .ok_or_else(|| invalid("required argument is not an integer")),
        other => Err(invalid(format!(
            "required argument is not an integer: {other}"
        ))),
    }
}

fn float_of(v: &Value) -> Result<f64, CodecError> {
    match v {
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| invalid("required argument is not a float")),
        other => Err(invalid(format!(
            "required argument is not a float: {other}"
        ))),
    }
}

fn pack_scalar(sc: Sc, v: &Value, out: &mut [u8]) -> Result<(), CodecError> {
    macro_rules! int {
        ($t:ty) => {{
            let x = int_of(v)?;
            let x = <$t>::try_from(x).map_err(|_| {
                invalid(format!("argument out of range for {}: {x}", stringify!($t)))
            })?;
            out.copy_from_slice(&x.to_le_bytes());
        }};
    }
    match sc {
        Sc::Bool => out[0] = u8::from(truthy(v)),
        Sc::U8 => int!(u8),
        Sc::I8 => int!(i8),
        Sc::U16 => int!(u16),
        Sc::I16 => int!(i16),
        Sc::U32 => int!(u32),
        Sc::I32 => int!(i32),
        Sc::U64 => int!(u64),
        Sc::I64 => int!(i64),
        Sc::F32 => {
            let x = float_of(v)?;
            let y = x as f32;
            if y.is_infinite() && x.is_finite() {
                return Err(invalid("float too large to pack with f format"));
            }
            out.copy_from_slice(&y.to_le_bytes());
        }
        Sc::F64 => out.copy_from_slice(&float_of(v)?.to_le_bytes()),
    }
    Ok(())
}

/// Pack a struct value (missing fields are zero).
fn pack_struct(si: usize, v: &Value) -> Result<Vec<u8>, CodecError> {
    let d = &STRUCTS[si];
    match d.name {
        "AssetHash" => {
            let raw = match v {
                Value::String(s) => unhex(s).ok_or_else(|| {
                    invalid("AssetHash: non-hexadecimal number found in fromhex() arg")
                })?,
                Value::Null => Vec::new(),
                other => bytes_of(other)?,
            };
            if raw.len() != 64 {
                return Err(invalid(
                    "AssetHash must be a 64-byte sha512 (128 hex chars)",
                ));
            }
            return Ok(raw);
        }
        "Uuid" => {
            let n = if truthy(v) {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                parse_uuid(&s)
                    .ok_or_else(|| invalid("Uuid: badly formed hexadecimal UUID string"))?
            } else {
                0
            };
            let mut out = Vec::with_capacity(16);
            out.extend_from_slice(&(n as u64).to_le_bytes());
            out.extend_from_slice(&((n >> 64) as u64).to_le_bytes());
            return Ok(out);
        }
        _ => {}
    }
    let empty = Map::new();
    let obj = match v {
        Value::Object(o) => o,
        v if !truthy(v) => &empty,
        other => {
            return Err(invalid(format!(
                "{}: expected an object, got {other}",
                d.name
            )));
        }
    };
    let mut out = vec![0u8; d.size];
    for f in d.fields {
        match f.ty {
            Ty::Struct(s) => {
                let sb = pack_struct(s, obj.get(f.name).unwrap_or(&Value::Null))?;
                out[f.offset..f.offset + sb.len()].copy_from_slice(&sb);
            }
            Ty::Scalar(sc) => {
                let zero = Value::from(0);
                let x = obj.get(f.name).unwrap_or(&zero);
                pack_scalar(sc, x, &mut out[f.offset..f.offset + sc.size()])?;
            }
            other => return Err(invalid(format!("struct field of type {other:?}"))),
        }
    }
    Ok(out)
}

fn bytes_of(v: &Value) -> Result<Vec<u8>, CodecError> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| {
                int_of(x).and_then(|n| {
                    u8::try_from(n).map_err(|_| invalid("bytes must be in range(0, 256)"))
                })
            })
            .collect(),
        other => Err(invalid(format!("expected bytes, got {other}"))),
    }
}

enum Item<'a> {
    Scalar(Sc, &'a Value),
    Tag(u8),
    Struct(Vec<u8>),
    Off(Deferred<'a>),
}

enum Deferred<'a> {
    Union(Variant, &'a Value),
    String(&'a Value),
    Table(usize, &'a Value),
    Vector(El, &'a Value),
}

struct Inline<'a> {
    slot: usize,
    size: usize,
    align: usize,
    item: Item<'a>,
}

struct Builder {
    buf: Vec<u8>,
}

impl Builder {
    fn pad_to(&mut self, a: usize, extra: usize) {
        while !(self.buf.len() + extra).is_multiple_of(a) {
            self.buf.push(0);
        }
    }

    fn patch(&mut self, at: usize, target: usize) {
        let rel = (target - at) as u32;
        self.buf[at..at + 4].copy_from_slice(&rel.to_le_bytes());
    }

    fn string(&mut self, v: &Value) -> Result<usize, CodecError> {
        let Value::String(s) = v else {
            return Err(invalid(format!("expected a string, got {v}")));
        };
        self.pad_to(4, 0);
        let p = self.buf.len();
        self.buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
        Ok(p)
    }

    fn vector(&mut self, el: El, v: &Value) -> Result<usize, CodecError> {
        let Value::Array(items) = v else {
            return Err(invalid(format!("expected a list, got {v}")));
        };
        let count = (items.len() as u32).to_le_bytes();
        match el {
            El::Struct(s) => {
                let a = STRUCTS[s].align.max(4);
                self.pad_to(4, 0);
                self.pad_to(a, 4);
                let p = self.buf.len();
                self.buf.extend_from_slice(&count);
                for it in items {
                    let packed = pack_struct(s, it)?;
                    self.buf.extend_from_slice(&packed);
                }
                Ok(p)
            }
            El::Table(_) | El::String => {
                self.pad_to(4, 0);
                let p = self.buf.len();
                self.buf.extend_from_slice(&count);
                self.buf.resize(self.buf.len() + 4 * items.len(), 0);
                for (i, it) in items.iter().enumerate() {
                    let c = match el {
                        El::Table(t) => self.table(t, it)?,
                        _ => self.string(it)?,
                    };
                    self.patch(p + 4 + 4 * i, c);
                }
                Ok(p)
            }
            El::Scalar(sc) => {
                let sz = sc.size();
                self.pad_to(4, 0);
                self.pad_to(sz.max(4), 4);
                let p = self.buf.len();
                self.buf.extend_from_slice(&count);
                let mut tmp = [0u8; 8];
                for it in items {
                    pack_scalar(sc, it, &mut tmp[..sz])?;
                    self.buf.extend_from_slice(&tmp[..sz]);
                }
                Ok(p)
            }
        }
    }

    fn table(&mut self, ti: usize, v: &Value) -> Result<usize, CodecError> {
        let def = &TABLES[ti];
        let Value::Object(obj) = v else {
            return Err(invalid(format!(
                "{}: expected an object, got {v}",
                def.name
            )));
        };
        let mut extra: Vec<&str> = obj
            .keys()
            .map(String::as_str)
            .filter(|k| {
                !def.fields.iter().any(|f| {
                    f.name == *k
                        || (matches!(f.ty, Ty::Union(_)) && k.strip_suffix("Type") == Some(f.name))
                })
            })
            .collect();
        if !extra.is_empty() {
            extra.sort_unstable();
            let list: Vec<String> = extra.iter().map(|k| format!("'{k}'")).collect();
            return Err(invalid(format!(
                "{}: unknown fields [{}]",
                def.name,
                list.join(", ")
            )));
        }
        let mut inline: Vec<Inline<'_>> = Vec::new();
        for f in def.fields {
            let Some(v) = obj.get(f.name).filter(|v| !v.is_null()) else {
                continue;
            };
            match f.ty {
                Ty::Union(u) => {
                    let ty = v.get("type").and_then(Value::as_str).unwrap_or_default();
                    let variants = UNIONS[u].variants;
                    let Some(idx) = variants.iter().position(|(n, _)| *n == ty) else {
                        let why = if ty == UNKNOWN {
                            " (unknown ops cannot be re-encoded)"
                        } else {
                            ""
                        };
                        return Err(invalid(format!(
                            "{}.{}: bad union type {ty}{why}",
                            def.name, f.name
                        )));
                    };
                    let tslot = def
                        .fields
                        .iter()
                        .find(|t| t.name.strip_suffix("Type") == Some(f.name))
                        .map_or(f.slot - 1, |t| t.slot);
                    let tag = u8::try_from(idx + 1).map_err(|_| invalid("union tag overflow"))?;
                    inline.push(Inline {
                        slot: tslot,
                        size: 1,
                        align: 1,
                        item: Item::Tag(tag),
                    });
                    let value = v.get("value").unwrap_or(&Value::Null);
                    inline.push(Inline {
                        slot: f.slot,
                        size: 4,
                        align: 4,
                        item: Item::Off(Deferred::Union(variants[idx].1, value)),
                    });
                }
                Ty::Scalar(sc) => inline.push(Inline {
                    slot: f.slot,
                    size: sc.size(),
                    align: sc.size(),
                    item: Item::Scalar(sc, v),
                }),
                Ty::Struct(s) => inline.push(Inline {
                    slot: f.slot,
                    size: STRUCTS[s].size,
                    align: STRUCTS[s].align,
                    item: Item::Struct(pack_struct(s, v)?),
                }),
                Ty::String => inline.push(Inline {
                    slot: f.slot,
                    size: 4,
                    align: 4,
                    item: Item::Off(Deferred::String(v)),
                }),
                Ty::Table(t) => inline.push(Inline {
                    slot: f.slot,
                    size: 4,
                    align: 4,
                    item: Item::Off(Deferred::Table(t, v)),
                }),
                Ty::Vector(el) => inline.push(Inline {
                    slot: f.slot,
                    size: 4,
                    align: 4,
                    item: Item::Off(Deferred::Vector(el, v)),
                }),
            }
        }
        inline.sort_by_key(|i| std::cmp::Reverse(i.align)); // stable sort
        let maxa = inline.iter().map(|i| i.align).max().unwrap_or(4).max(4);
        let mut layout = Vec::with_capacity(inline.len());
        let mut cur = 4usize;
        for i in inline {
            while !cur.is_multiple_of(i.align) {
                cur += 1;
            }
            let size = i.size;
            layout.push((cur, i));
            cur += size;
        }
        let tsize = cur;
        let nslots = layout.iter().map(|(_, i)| i.slot + 1).max().unwrap_or(0);
        let mut vt = vec![0u16; 2 + nslots];
        vt[0] = (4 + 2 * nslots) as u16;
        vt[1] = u16::try_from(tsize).map_err(|_| invalid("table too large"))?;
        for (off, i) in &layout {
            vt[2 + i.slot] = *off as u16;
        }
        self.pad_to(2, 0);
        let vpos = self.buf.len();
        for x in &vt {
            self.buf.extend_from_slice(&x.to_le_bytes());
        }
        self.pad_to(maxa, 0);
        let tpos = self.buf.len();
        let mut body = vec![0u8; tsize];
        body[..4].copy_from_slice(&((tpos - vpos) as i32).to_le_bytes());
        let mut deferred = Vec::new();
        for (off, i) in layout {
            match i.item {
                Item::Struct(bytes) => body[off..off + i.size].copy_from_slice(&bytes),
                Item::Tag(t) => body[off] = t,
                Item::Scalar(sc, v) => pack_scalar(sc, v, &mut body[off..off + i.size])?,
                Item::Off(d) => deferred.push((tpos + off, d)),
            }
        }
        self.buf.extend_from_slice(&body);
        for (at, d) in deferred {
            let c = match d {
                Deferred::Union(Variant::Table(t), v) => self.table(t, v)?,
                Deferred::Union(Variant::Struct(s), v) => {
                    self.pad_to(STRUCTS[s].align.max(4), 0);
                    let p = self.buf.len();
                    let packed = pack_struct(s, v)?;
                    self.buf.extend_from_slice(&packed);
                    p
                }
                Deferred::String(v) => self.string(v)?,
                Deferred::Table(t, v) => self.table(t, v)?,
                Deferred::Vector(el, v) => self.vector(el, v)?,
            };
            self.patch(at, c);
        }
        Ok(tpos)
    }
}

/// Encode `obj` as a FlatBuffer whose root is `table`.
pub fn encode(table: &str, obj: &Value) -> Result<Vec<u8>, CodecError> {
    let ti = crate::schema::table(table).ok_or_else(|| CodecError::UnknownTable(table.into()))?;
    let mut b = Builder { buf: vec![0; 4] };
    let root = b.table(ti, obj)?;
    b.patch(0, root);
    b.pad_to(4, 0);
    Ok(b.buf)
}

// ----------------------------------------------------------------------------------- helpers

pub fn hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
}

/// Hex decoding: pairs of hex digits, ASCII whitespace allowed between pairs.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect()
}

/// Canonical lowercase 8-4-4-4-12 form.
pub fn uuid_string(n: u128) -> String {
    let h = format!("{n:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Lenient UUID parsing: optional `urn:` / `uuid:` prefix and braces, hyphens anywhere.
pub fn parse_uuid(s: &str) -> Option<u128> {
    let s = s.strip_prefix("urn:").unwrap_or(s);
    let s = s.strip_prefix("uuid:").unwrap_or(s);
    let s = s.trim_matches(|c| c == '{' || c == '}');
    let h: String = s.chars().filter(|&c| c != '-').collect();
    if h.len() != 32 || !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(&h, 16).ok()
}

/// `{"bitsR", "bitsG", "bitsB", "bitsA"}` colour object.
pub fn color(r: u8, g: u8, b: u8, a: u8) -> Value {
    serde_json::json!({"bitsR": r, "bitsG": g, "bitsB": b, "bitsA": a})
}

/// `#RRGGBB` for a colour object (alpha ignored).
pub fn color_hex(c: &Value) -> Option<String> {
    let o = c.as_object().filter(|o| !o.is_empty())?;
    let ch = |k: &str| o.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(format!(
        "#{:02X}{:02X}{:02X}",
        ch("bitsR"),
        ch("bitsG"),
        ch("bitsB")
    ))
}
