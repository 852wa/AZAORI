use azaori::conformance::{difference, run_case};
use serde_json::{json, Value};
use std::{fs, path::Path};

#[test]
fn runtime_matches_reference_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("conformance/manifest.json")).unwrap())
            .unwrap();
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for suite in manifest["suites"].as_array().unwrap() {
        if suite["format"] != "case-corpus" {
            continue;
        }
        let path = root
            .join("conformance")
            .join(suite["path"].as_str().unwrap());
        let fixture: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            if case["mode"] != "run" {
                continue;
            }
            checked += 1;
            let result = std::panic::catch_unwind(|| run_case(case));
            let mismatch = match result {
                Ok(actual) => difference(&case["expected"], &actual, "$"),
                Err(payload) => Some(format!(
                    "panic: {}",
                    payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic")
                )),
            };
            if let Some(diff) = mismatch {
                failures.push(json!({"id":case["id"],"source":case["source"],"diff":diff}));
            }
            if checked.is_multiple_of(1000) {
                eprintln!(
                    "runtime conformance: {checked} checked, {} mismatches",
                    failures.len()
                );
            }
        }
    }
    let target = root.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(
        target.join("runtime-failures.json"),
        serde_json::to_string_pretty(&failures).unwrap(),
    )
    .unwrap();
    eprintln!(
        "runtime conformance: {checked} checked, {} passed, {} failed",
        checked - failures.len(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} runtime mismatches; details in target/runtime-failures.json",
        failures.len()
    );
}
