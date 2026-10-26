//! Lamport ids: `Id{site, timestamp}` for ops/entities, `SeqId{site, timestamp, index}` for
//! characters and pages. Rendered as `site:ts[:idx]`.

use nb_api::{NbError, Result};
use serde_json::{Value, json};

pub type Id = (u64, u64);
pub type SeqId = (u64, u64, u64);

pub fn id_of(v: &Value) -> Option<Id> {
    Some((v.get("site")?.as_u64()?, v.get("timestamp")?.as_u64()?))
}

pub fn seq_of(v: &Value) -> Option<SeqId> {
    let (s, t) = id_of(v)?;
    Some((s, t, v.get("index").and_then(Value::as_u64).unwrap_or(0)))
}

pub fn idd(i: Id) -> Value {
    json!({"site": i.0, "timestamp": i.1})
}

pub fn seqd(s: SeqId) -> Value {
    json!({"site": s.0, "timestamp": s.1, "index": s.2})
}

pub fn fmt_id(i: Id) -> String {
    format!("{}:{}", i.0, i.1)
}

pub fn fmt_seq(s: SeqId) -> String {
    format!("{}:{}:{}", s.0, s.1, s.2)
}

pub fn parse_id(s: &str) -> Result<Id> {
    let bad = || NbError::usage(format!("bad id '{s}' (expected SITE:TIMESTAMP, e.g. 0:6)"));
    let (a, b) = s.split_once(':').ok_or_else(bad)?;
    Ok((a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?))
}

/// Order an RGA: each node is inserted right after its anchor (`None` = head); siblings with
/// the same anchor sort newest first (timestamp, then site, then lower index).
pub fn linearize(nodes: &[(SeqId, Option<SeqId>)]) -> Vec<SeqId> {
    use std::collections::HashMap;
    let mut kids: HashMap<Option<SeqId>, Vec<SeqId>> = HashMap::new();
    for &(n, a) in nodes {
        kids.entry(a).or_default().push(n);
    }
    for v in kids.values_mut() {
        v.sort_by(|a, b| (b.1, b.0, a.2).cmp(&(a.1, a.0, b.2)));
    }
    let mut out = Vec::with_capacity(nodes.len());
    let mut st: Vec<SeqId> = kids
        .get(&None)
        .map(|v| v.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(n) = st.pop() {
        out.push(n);
        if let Some(k) = kids.get(&Some(n)) {
            st.extend(k.iter().rev());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rga_newest_sibling_first() {
        let a = (1, 1, 0);
        let b = (1, 1, 1);
        let c = (2, 5, 0);
        // "ab" then "c" inserted after a: a c b
        assert_eq!(
            linearize(&[(a, None), (b, Some(a)), (c, Some(a))]),
            vec![a, c, b]
        );
        assert_eq!(parse_id("0:6").unwrap(), (0, 6));
        assert!(parse_id("x").is_err());
    }
}
