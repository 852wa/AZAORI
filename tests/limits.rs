//! 深い式・大きな値でも、処理系が落ちずにエラーを返すこと（上限の確認）。
//! テストのスレッドは CLI・WASM と同じ 64MB のスタックで動く（.cargo/config.toml の RUST_MIN_STACK）。
use azaori::{analysis, expand, interpreter::Interpreter, parser, readings};

fn everything(source: &str) -> Result<Vec<String>, String> {
    let program = parser::parse(source).map_err(|e| e.message)?;
    readings::read_program(&program);
    readings::read_program_marked(&program);
    expand::expand_program(&program);
    analysis::analyze_program(&program);
    let mut interpreter = Interpreter::default();
    interpreter.trace = Some(Default::default());
    interpreter
        .run(&program)
        .map(|rows| {
            rows.iter()
                .map(|(_, a)| azaori::values::format_value(a))
                .collect()
        })
        .map_err(|e| e.message)
}

#[test]
fn deep_parentheses_are_an_error_not_a_crash() {
    let deep = format!("{}1{}", "（".repeat(10_000), "）".repeat(10_000));
    assert!(everything(&deep)
        .unwrap_err()
        .contains("入れ子が深すぎます"));
    let ok = format!("{}1{} 足 1", "（".repeat(100), "）".repeat(100));
    assert_eq!(everything(&ok).unwrap(), ["2"]);
    let anon = format!("1 {}足 1{}", "各（".repeat(10_000), "）".repeat(10_000));
    assert!(everything(&anon)
        .unwrap_err()
        .contains("入れ子が深すぎます"));
    let signs = format!("1 足 {}1", "-".repeat(5_000));
    assert!(everything(&signs).is_err());
}

#[test]
fn long_symbol_chains_are_limited() {
    let chain = |n: usize| format!("{} 足 0", vec!["1"; n + 1].join(" + "));
    assert_eq!(everything(&chain(1000)).unwrap(), ["1001"]);
    assert!(everything(&chain(1001))
        .unwrap_err()
        .contains("記号の計算が多すぎます"));
    let power = format!("{}1", "1 ^ ".repeat(999));
    assert_eq!(everything(&power).unwrap(), ["1"]);
    // 入れ子の上限と記号の上限を合わせた、いちばん深い式
    let mut worst = String::from("1");
    for _ in 0..99 {
        worst = format!("（{worst} + 1）");
    }
    let worst = format!("{worst}{} 足 0", " + 1".repeat(900));
    assert_eq!(everything(&worst).unwrap(), ["1000"]);
}
