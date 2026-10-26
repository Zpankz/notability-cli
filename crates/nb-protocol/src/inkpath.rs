//! Codec for Notability's encoded ink paths (`encodedCenterPath`, `encodedCustomPath`,
//! `encodedFillPath`, `encodedCenterPathElements`).
//!
//! Layout (little-endian): byte 0 = `version << 3 | precision` (version 0; precision 0 = f16,
//! 1 = f32), u16 element count, then per element a u8 type and its points. Types 0..=3 are the
//! attributed cubic/quadratic/line/moveto and append f16 width, f16 force, u8 altitude
//! (π/2·v/255) and u8 azimuth (2π·v/256); types 4..=7 are the plain variants.

use std::f64::consts::PI;

use half::f16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PathError(pub String);

fn err<T>(s: impl Into<String>) -> Result<T, PathError> {
    Err(PathError(s.into()))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PathElement {
    pub kind: u8,
    pub points: Vec<(f64, f64)>,
    pub width: Option<f64>,
    pub force: Option<f64>,
    pub altitude: Option<f64>,
    pub azimuth: Option<f64>,
}

impl PathElement {
    pub fn is_moveto(&self) -> bool {
        matches!(self.kind, 3 | 7)
    }
    /// End point (the last point for every element type).
    pub fn end(&self) -> (f64, f64) {
        self.points.last().copied().unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub precision: u8,
    pub elements: Vec<PathElement>,
}

/// Number of points for an element type, or `None` if unknown.
pub fn npts(kind: u8) -> Option<usize> {
    match kind {
        0 | 4 => Some(3),
        1 | 5 => Some(2),
        2 | 3 | 6 | 7 => Some(1),
        _ => None,
    }
}

fn attributed(kind: u8) -> bool {
    kind <= 3
}

fn rd_f16(b: &[u8], p: usize) -> f64 {
    f16::from_bits(u16::from_le_bytes([b[p], b[p + 1]])).to_f64()
}

fn rd_f32(b: &[u8], p: usize) -> f64 {
    f64::from(f32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]))
}

pub fn decode(b: &[u8]) -> Result<Path, PathError> {
    if b.is_empty() {
        return Ok(Path::default());
    }
    if b.len() < 3 {
        return err("path too short");
    }
    let (version, precision) = (b[0] >> 3, b[0] & 7);
    if version > 0 {
        return err(format!("unsupported path encoding version {version}"));
    }
    if precision > 1 {
        return err(format!("unknown precision {precision}"));
    }
    let count = u16::from_le_bytes([b[1], b[2]]);
    let psize = if precision == 0 { 4 } else { 8 };
    let mut p = 3;
    let mut out = Path {
        precision,
        elements: Vec::with_capacity(count.into()),
    };
    for _ in 0..count {
        let Some(&kind) = b.get(p) else {
            return err("truncated path");
        };
        p += 1;
        let Some(n) = npts(kind) else {
            return err(format!("unexpected path element type {kind}"));
        };
        let mut el = PathElement {
            kind,
            points: Vec::with_capacity(n),
            ..Default::default()
        };
        for _ in 0..n {
            if p + psize > b.len() {
                return err("truncated point");
            }
            el.points.push(if precision == 0 {
                (rd_f16(b, p), rd_f16(b, p + 2))
            } else {
                (rd_f32(b, p), rd_f32(b, p + 4))
            });
            p += psize;
        }
        if attributed(kind) {
            if p + 6 > b.len() {
                return err("truncated attributes");
            }
            el.width = Some(rd_f16(b, p));
            el.force = Some(rd_f16(b, p + 2));
            el.altitude = Some(PI / 2.0 * f64::from(b[p + 4]) / 255.0);
            el.azimuth = Some(2.0 * PI * f64::from(b[p + 5]) / 256.0);
            p += 6;
        }
        out.elements.push(el);
    }
    Ok(out)
}

fn put_f16(out: &mut Vec<u8>, v: f64) -> Result<(), PathError> {
    let h = f16::from_f64(v);
    if h.is_infinite() && v.is_finite() {
        return err("float too large to pack with e format");
    }
    out.extend_from_slice(&h.to_bits().to_le_bytes());
    Ok(())
}

fn put_f32(out: &mut Vec<u8>, v: f64) -> Result<(), PathError> {
    let f = v as f32;
    if f.is_infinite() && v.is_finite() {
        return err("float too large to pack with f format");
    }
    out.extend_from_slice(&f.to_le_bytes());
    Ok(())
}

fn angle_byte(v: f64, full: f64) -> u8 {
    ((v * full).round_ties_even() as i64 & 0xFF) as u8
}

/// Inverse of [`decode`]. Without an explicit precision, f32 is chosen when any coordinate of
/// any element's *first* point exceeds 256 in magnitude (matching the web client).
pub fn encode(elements: &[PathElement], precision: Option<u8>) -> Result<Vec<u8>, PathError> {
    let precision = precision.unwrap_or_else(|| {
        let big = elements
            .iter()
            .filter_map(|e| e.points.first())
            .any(|&(x, y)| x.abs() > 256.0 || y.abs() > 256.0);
        u8::from(big)
    });
    let count =
        u16::try_from(elements.len()).map_err(|_| PathError("too many path elements".into()))?;
    let mut out = vec![precision];
    out.extend_from_slice(&count.to_le_bytes());
    for e in elements {
        out.push(e.kind);
        for &(x, y) in &e.points {
            if precision == 0 {
                put_f16(&mut out, x)?;
                put_f16(&mut out, y)?;
            } else {
                put_f32(&mut out, x)?;
                put_f32(&mut out, y)?;
            }
        }
        if attributed(e.kind) {
            put_f16(&mut out, e.width.unwrap_or(0.0))?;
            put_f16(&mut out, e.force.unwrap_or(0.0))?;
            out.push(angle_byte(e.altitude.unwrap_or(0.0), 255.0 / (PI / 2.0)));
            out.push(angle_byte(e.azimuth.unwrap_or(0.0), 256.0 / (2.0 * PI)));
        }
    }
    Ok(out)
}
