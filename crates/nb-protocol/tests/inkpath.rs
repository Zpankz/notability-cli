use std::fs;

use nb_protocol::inkpath::{self, PathElement};
use serde_json::Value;

fn el(v: &Value) -> PathElement {
    let f = |k: &str| v.get(k).and_then(Value::as_f64);
    PathElement {
        kind: v["type"].as_u64().unwrap() as u8,
        points: v["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
            .collect(),
        width: f("width"),
        force: f("force"),
        altitude: f("altitude"),
        azimuth: f("azimuth"),
    }
}

fn bytes(v: &Value) -> Vec<u8> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect()
}

#[test]
fn inkpath_goldens() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/goldens/inkpath");
    let mut n = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let g: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let raw = bytes(&g["bytes"]);
        if let Some(e) = g.get("error") {
            assert_eq!(
                inkpath::decode(&raw).unwrap_err().0,
                e.as_str().unwrap(),
                "{path:?}"
            );
        } else {
            let els: Vec<PathElement> = g["elements"].as_array().unwrap().iter().map(el).collect();
            assert_eq!(inkpath::encode(&els, None).unwrap(), raw, "{path:?}");
            let d = inkpath::decode(&raw).unwrap();
            assert_eq!(u64::from(d.precision), g["precision"].as_u64().unwrap());
            let want: Vec<PathElement> = g["decoded"].as_array().unwrap().iter().map(el).collect();
            assert_eq!(d.elements, want, "{path:?}");
        }
        n += 1;
    }
    assert_eq!(n, 9);
}

#[test]
fn empty_and_overflow() {
    assert!(inkpath::decode(&[]).unwrap().elements.is_empty());
    let big = PathElement {
        kind: 6,
        points: vec![(1.0, 70000.0)],
        ..Default::default()
    };
    assert!(inkpath::encode(&[big], Some(0)).is_err());
}
