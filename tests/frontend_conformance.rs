use azaori::{expand, parser, readings};
use serde_json::{json, Value};
use std::{fs, path::Path};

#[test]
fn frontend_matches_reference_corpus() {
    // 照合する集合は conformance/manifest.json に書いてある（実行の照合と同じ）
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("conformance/manifest.json")).unwrap())
            .unwrap();
    let mut cases = Vec::new();
    for suite in manifest["suites"].as_array().unwrap() {
        if suite["format"] != "case-corpus" {
            continue;
        }
        let path = root
            .join("conformance")
            .join(suite["path"].as_str().unwrap());
        let fixture: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        cases.extend(fixture["cases"].as_array().unwrap().iter().cloned());
    }
    let mut checked = [0usize; 3];
    let mut failures = Vec::new();
    for case in &cases {
        let mode = case["mode"].as_str().unwrap();
        let slot = match mode {
            "parse" => 0,
            "readings" => 1,
            "expand" => 2,
            _ => continue,
        };
        checked[slot] += 1;
        let source = case["source"].as_str().unwrap();
        let expected = &case["expected"];
        let actual = match parser::parse(source) {
            Err(error) => json!({"error": error}),
            Ok(program) => match mode {
                "parse" => json!({"ok": true}),
                "readings" => json!({"lines": readings::read_program(&program)}),
                "expand" => json!({"lines": expand::expand_program(&program)}),
                _ => unreachable!(),
            },
        };
        let equal = if let Some(error) = expected.get("error").filter(|v| !v.is_null()) {
            ["kind", "line", "col", "end"].iter().all(|key| {
                error
                    .get(*key)
                    .is_none_or(|value| actual["error"].get(*key) == Some(value))
            })
        } else {
            actual == *expected
        };
        if !equal {
            failures.push(format!(
                "{} {}: {:?}\nexpected: {}\nactual:   {}",
                case["id"], mode, source, expected, actual
            ));
        }
    }
    eprintln!(
        "frontend checked: parse={}, readings={}, expand={}; failures={}",
        checked[0],
        checked[1],
        checked[2],
        failures.len()
    );
    for failure in failures.iter().take(80) {
        eprintln!("{failure}");
    }
    assert!(
        failures.is_empty(),
        "{} frontend conformance failures",
        failures.len()
    );
}
