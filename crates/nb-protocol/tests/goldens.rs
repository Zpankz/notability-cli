//! Byte-for-byte parity with frozen reference encodings (`goldens/`): every table, struct and
//! union variant of the schema, empty and fully populated.

use std::path::{Path, PathBuf};

use nb_protocol::codec::{decode, encode};
use serde_json::Value;

fn dir(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("goldens")
        .join(sub)
}

fn cases() -> Vec<(String, Value, Vec<u8>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir("ops")).expect("goldens/ops") {
        let p = e.expect("entry").path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let j: Value =
            serde_json::from_str(&std::fs::read_to_string(&p).expect("read")).expect("json");
        let bin = std::fs::read(p.with_extension("bin")).expect("bin");
        out.push((
            p.file_stem().unwrap().to_string_lossy().into_owned(),
            j,
            bin,
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        out.len() > 250,
        "expected the full golden corpus, found {}",
        out.len()
    );
    out
}

#[test]
fn encode_is_byte_equal() {
    let mut bad = Vec::new();
    for (name, j, bin) in cases() {
        let table = j["table"].as_str().unwrap();
        match encode(table, &j["spec"]) {
            Ok(b) if b == bin => {}
            Ok(b) => bad.push(format!("{name}: {} bytes vs golden {}", b.len(), bin.len())),
            Err(e) => bad.push(format!("{name}: {e}")),
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn decode_is_spec_equal() {
    let mut bad = Vec::new();
    for (name, j, bin) in cases() {
        let table = j["table"].as_str().unwrap();
        let src = j.get("source_hex").and_then(Value::as_str).map(|h| {
            (0..h.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
                .collect::<Vec<u8>>()
        });
        let input = src.as_deref().unwrap_or(&bin);
        match decode(table, input) {
            Ok(v) if v == j["decoded"] => {}
            Ok(v) => bad.push(format!("{name}:\n  rust   {v}\n  golden {}", j["decoded"])),
            Err(e) => bad.push(format!("{name}: {e}")),
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn decode_encode_round_trip() {
    for (name, j, bin) in cases() {
        let table = j["table"].as_str().unwrap();
        let v = decode(table, &bin).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(encode(table, &v).unwrap(), bin, "{name}");
    }
}

#[test]
fn synthetic_bundle_fixture() {
    let raw = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic_bundle.bin"),
    )
    .unwrap();
    let v = decode("NoteBundle", &raw).unwrap();
    assert_eq!(v["ops"].as_array().unwrap().len(), 10);
    let again = encode("NoteBundle", &v).unwrap();
    assert_eq!(decode("NoteBundle", &again).unwrap(), v);
}
