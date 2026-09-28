//! 手で書いた仕様（conformance/spec.json）を照合する。
//! Rust 版が本体になってからの言語の変更は、まずここに「書き方 → 結果」を足してから実装する。
use azaori::{interpreter::Interpreter, parser::parse, values::format_value};
use serde_json::Value;

#[test]
fn hand_written_spec_cases() {
    let text = include_str!("../conformance/spec.json");
    let spec: Value = serde_json::from_str(text).expect("spec.json は正しい JSON");
    let mut failures = Vec::new();
    for case in spec["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap_or("?");
        let source = case["source"].as_str().expect("source");
        let outcome = parse(source).and_then(|program| {
            let mut interpreter = Interpreter::default();
            interpreter.run(&program).map(|rows| {
                rows.iter()
                    .map(|(_, a)| format_value(a))
                    .collect::<Vec<_>>()
            })
        });
        match (outcome, case.get("results"), case.get("error")) {
            (Ok(actual), Some(expected), _) => {
                let expected: Vec<String> = serde_json::from_value(expected.clone()).unwrap();
                if actual != expected {
                    failures.push(format!("{name}: 結果 {actual:?}（期待 {expected:?}）"));
                }
            }
            (Err(error), _, Some(expected)) => {
                if error.message != expected.as_str().unwrap_or("") {
                    failures.push(format!(
                        "{name}: エラー「{}」（期待「{expected}」）",
                        error.message
                    ));
                }
            }
            (Ok(actual), None, _) => failures.push(format!("{name}: エラーのはずが {actual:?}")),
            (Err(error), _, None) => failures.push(format!("{name}: エラー「{}」", error.message)),
        }
    }
    assert!(
        failures.is_empty(),
        "仕様と違う結果:\n{}",
        failures.join("\n")
    );
}
