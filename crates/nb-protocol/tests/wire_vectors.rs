//! Wire vectors: every op kind the web editor sends, as an `AppendOpsEvent`. The codec must re-encode each recorded op
//! byte-for-byte and decode the bytes back to it (floats within 1e-3).

use std::path::Path;

use nb_protocol::codec::{decode, encode, unhex};
use serde_json::{Map, Value, json};

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire")
}

fn vectors() -> Vec<(String, Value, Vec<u8>)> {
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(root().join("ops"))
        .expect("ops")
        .map(|e| e.expect("entry").path())
        .collect();
    files.sort();
    for p in files {
        let d: Value = serde_json::from_slice(&std::fs::read(&p).expect("read")).expect("json");
        let hex = d["append_ops_event_hex"].as_str().expect("hex");
        out.push((
            p.file_name().unwrap().to_string_lossy().into_owned(),
            d["op"].clone(),
            unhex(hex).expect("unhex"),
        ));
    }
    let web: Value = serde_json::from_slice(
        &std::fs::read(root().join("web_ui/web_editor_ops.json")).expect("web"),
    )
    .expect("json");
    for (i, d) in web["ops"].as_array().expect("ops").iter().enumerate() {
        let name = format!(
            "web_ui[{i}]:{}",
            d["op"]["payload"]["type"].as_str().unwrap_or("?")
        );
        out.push((
            name,
            d["op"].clone(),
            unhex(d["append_ops_event_hex"].as_str().expect("hex")).expect("unhex"),
        ));
    }
    out
}

/// `{"hex": "..."}` in a recorded op stands for a `[ubyte]` vector, which this codec models as
/// a list of byte values.
fn unhex_values(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("hex") => Value::Array(
            unhex(m["hex"].as_str().unwrap_or_default())
                .expect("hex")
                .into_iter()
                .map(Value::from)
                .collect(),
        ),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), unhex_values(x)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(unhex_values).collect()),
        _ => v.clone(),
    }
}

fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => x
            .iter()
            .all(|(k, v)| close(v, y.get(k).unwrap_or(&Value::Null))),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| close(p, q))
        }
        (Value::Number(x), Value::Number(y)) if x.is_f64() || y.is_f64() => {
            let (p, q) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            (p - q).abs() <= 1e-3 * p.abs().max(1.0)
        }
        _ => a == b,
    }
}

#[test]
fn wire_vectors_roundtrip() {
    let vs = vectors();
    assert!(vs.len() >= 50, "found {}", vs.len());
    let mut failures = Vec::new();
    for (name, op, raw) in &vs {
        let op = unhex_values(op);
        match encode("AppendOpsEvent", &json!({"ops": [op]})) {
            Ok(b) if &b == raw => {}
            Ok(_) => failures.push(format!("{name}: encode differs")),
            Err(e) => failures.push(format!("{name}: encode error {e}")),
        }
        match decode("AppendOpsEvent", raw) {
            Ok(d) if close(&op, &d["ops"][0]) => {}
            Ok(d) => failures.push(format!("{name}: decode differs: {}", d["ops"][0])),
            Err(e) => failures.push(format!("{name}: decode error {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} vectors failed:\n{}",
        failures.len(),
        vs.len(),
        failures.join("\n")
    );
}
