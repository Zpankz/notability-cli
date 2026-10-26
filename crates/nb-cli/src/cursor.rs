//! Opaque list cursors: base64url of `{offset, snapshot_version}`.

pub const STALE_CURSOR: &str = "STALE_CURSOR";

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nb_api::{NbError, Result};
use serde_json::{Value, json};

pub fn encode(offset: usize, snapshot: &str) -> String {
    let raw = json!({"offset": offset, "snapshot_version": snapshot}).to_string();
    URL_SAFE_NO_PAD.encode(raw)
}

/// A cursor's `(offset, snapshot_version)`; garbage is a usage error.
pub fn parse(cursor: &str) -> Result<(usize, String)> {
    let bad = || NbError::usage("invalid --cursor (pass meta.next_cursor unchanged)");
    let raw = URL_SAFE_NO_PAD.decode(cursor.trim()).map_err(|_| bad())?;
    let v: Value = serde_json::from_slice(&raw).map_err(|_| bad())?;
    let offset = v["offset"].as_u64().ok_or_else(bad)? as usize;
    let snapshot = v["snapshot_version"].as_str().ok_or_else(bad)?;
    Ok((offset, snapshot.to_string()))
}

/// The offset a cursor points at; rejects cursors minted for a different snapshot.
pub fn decode(cursor: &str, snapshot: &str) -> Result<usize> {
    let (offset, minted) = parse(cursor)?;
    if minted != snapshot {
        return Err(NbError::usage(
            "stale --cursor: the list changed since it was issued; start again without --cursor",
        )
        .with_key(Some(STALE_CURSOR)));
    }
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let c = encode(50, "s1");
        assert_eq!(decode(&c, "s1").unwrap(), 50);
    }

    #[test]
    fn stale_and_garbage_are_usage_errors() {
        let c = encode(50, "s1");
        let e = decode(&c, "s2").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(e.key.as_deref(), Some("STALE_CURSOR"));
        assert_eq!(decode("!!", "s1").unwrap_err().exit_code(), 2);
    }
}
