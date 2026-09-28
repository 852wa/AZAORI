//! Scope and unwinding regression cases.
use azaori::{
    error::AzaError,
    interpreter::{Host, Interpreter},
    parser::parse,
    values::format_value,
};
use std::{cell::Cell, rc::Rc};

fn run(source: &str) -> Result<Vec<String>, AzaError> {
    let mut interpreter = Interpreter::default();
    interpreter
        .run(&parse(source)?)
        .map(|rows| rows.iter().map(|(_, a)| format_value(a)).collect())
}

#[test]
fn non_tail_recursion_returns_through_every_pending_addition() {
    let definition = "【f】とは これ 以下 0 真 0 偽 （1 足 （これ 引 1 【f】））\n";
    assert_eq!(
        run(&(definition.to_string() + "4999 【f】")).unwrap(),
        ["4999"]
    );
    let error = run(&(definition.to_string() + "5001 【f】")).unwrap_err();
    assert!(error.message.contains("呼び出しが深すぎます"));
    assert_eq!(error.line, Some(2));
}

#[test]
fn recursive_block_locals_restore_callers_and_global_bindings() {
    let source="【n】は 1000\n【和】とは 説\n  仮【n】は これ\n  仮【子】は 【n】 以下 0 真 0 偽 （【n】 引 1 【和】）\n  【子】 足 【n】。\n8 【和】\n【n】";
    assert_eq!(run(source).unwrap(), ["36", "1000"]);
}

#[test]
fn named_callee_cannot_read_callers_block_locals() {
    let source="【読む】とは これ 足 【保存】\n【計算】とは 説\n  仮【保存】は これ 自掛\n  1 【読む】。\n3 【計算】\n4 【計算】";
    let error = run(source).unwrap_err();
    assert_eq!(error.message, "【保存】はまだありません（【計算】の2行目）");
    assert_eq!(
        (error.line, error.col, error.end),
        (Some(5), Some(2), Some(6))
    );
}

#[test]
fn caught_recursive_failure_restores_outer_locals_and_position() {
    let source="【危険】とは 説\n  仮【x】は これ\n  これ 以下 0 真 （これ 割 0） 偽 （これ 引 1 【危険】）。\n【復旧】とは 説\n  仮【x】は これ 掛 10\n  仮【a】は これ 試【危険】 7\n  【a】 足 【x】 足 ここ。\n1 2 3 各【復旧】";
    assert_eq!(run(source).unwrap(), ["18 29 40"]);
}

#[test]
fn depth_error_can_be_caught_without_leaking_frames() {
    let source="【f】とは これ 以下 0 真 0 偽 （1 足 （これ 引 1 【f】））\n5001 試【f】 7\n1 2 各（足 ここ）";
    assert_eq!(run(source).unwrap(), ["7", "2 4"]);
}

#[test]
fn nested_anonymous_subjects_and_positions_are_restored() {
    assert_eq!(
        run("1 2 3 各（これ 各（足 （あれ 総足）） 足 （あれ 総足））").unwrap(),
        ["8 10 12"]
    );
    assert_eq!(
        run("10 20 各（これ 毎（足 それ 足 ここ） 7 総足 足 ここ）").unwrap(),
        ["19 30"]
    );
    assert_eq!(
        run("【f】とは これ 回 3 （足 ここ）\n10 20 各【f】").unwrap(),
        ["13 26"]
    );
}

#[test]
fn anonymous_dyadic_arguments_do_not_replace_named_arguments() {
    let source =
        "【計算】とは 説\n  仮【一】は これ 各（足 それ） 10\n  【一】 足 それ。\n1 2 【計算】 100";
    assert_eq!(run(source).unwrap(), ["111 112"]);
}

#[test]
fn branch_uses_original_subject_instead_of_computed_condition() {
    assert_eq!(
        run("【f】とは これ 足 10 超 10 真 （これ 足 1） 偽 （これ 引 1）\n2 【f】").unwrap(),
        ["3"]
    );
}

#[test]
fn attempt_propagates_end_but_catches_end_inside_array_branch() {
    let mut interpreter = Interpreter::default();
    let program = parse("【止】とは これ 真 終 偽 99\n1 試【止】 123\n999").unwrap();
    assert!(interpreter.run(&program).unwrap().is_empty());
    assert!(interpreter.ended);
    assert_eq!(interpreter.events.len(), 1);
    assert_eq!(interpreter.events[0].kind, "終");
    assert_eq!(
        run("【止】とは これ 真 終 偽 99\n1 0 試【止】 123\n999").unwrap(),
        ["123", "999"]
    );
}

#[test]
fn nested_block_error_keeps_inner_context_and_outer_call_site() {
    let source="【内側】とは 説\n  仮【x】は これ 足 1\n  【x】 割 0。\n【外側】とは これ 【内側】\n10 【外側】";
    let error = run(source).unwrap_err();
    assert_eq!(error.message, "0で割ることはできません（【内側】の2行目）");
    assert_eq!(
        (error.line, error.col, error.end),
        (Some(5), Some(3), Some(7))
    );
}

#[test]
fn failed_run_does_not_leave_local_or_position_scope_in_interpreter() {
    let mut interpreter = Interpreter::default();
    let source = "【失敗】とは 説\n  仮【秘密】は これ\n  これ 各（割 0）。\n1 2 【失敗】";
    assert!(interpreter.run(&parse(source).unwrap()).is_err());
    let error = interpreter.run(&parse("【秘密】").unwrap()).unwrap_err();
    assert_eq!(error.message, "【秘密】はまだありません");
    let error = interpreter
        .run(&parse("【位置】とは ここ\n1 【位置】").unwrap())
        .unwrap_err();
    assert!(error.message.contains("何番目でもありません"));
}

struct StopAfter {
    calls: Cell<usize>,
    enabled: Rc<Cell<bool>>,
}
impl Host for StopAfter {
    fn stopped(&self) -> bool {
        self.calls.set(self.calls.get() + 1);
        self.enabled.get() && self.calls.get() > 100
    }
}

#[test]
fn stopped_deep_call_is_not_swallowed_by_attempt() {
    let enabled = Rc::new(Cell::new(true));
    let mut interpreter = Interpreter::new(Box::new(StopAfter {
        calls: Cell::new(0),
        enabled: enabled.clone(),
    }));
    let source = "【f】とは 説\n  仮【n】は これ\n  これ 引 1 【f】。\n1 試【f】 7\n999";
    let error = interpreter.run(&parse(source).unwrap()).unwrap_err();
    assert_eq!(error.message, "実行を止めました");
    assert!(error.col.is_none());
    assert!(error.end.is_none());
    assert!(interpreter.results.is_empty());
    enabled.set(false);
    let result = interpreter
        .run(&parse("1 2 各（足 ここ）").unwrap())
        .unwrap();
    assert_eq!(format_value(&result[0].1), "2 4");
}

#[test]
fn shared_machine_regressions() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/machine_cases.json");
    let corpus: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in corpus["cases"].as_array().unwrap() {
        let actual = azaori::conformance::run_case(case);
        if let Some(difference) = azaori::conformance::difference(&case["expected"], &actual, "$") {
            panic!("{}: {difference}", case["origin"]);
        }
    }
}
