//! 中身の記録（プレイグラウンドの「中身」タブ）: これ・それ・あれ・ここ・仮の名前
use azaori::{interpreter::Interpreter, parser::parse};

/// 記録の1件: （行、場所、入った行、名前と値の組）
type Entry = (usize, String, usize, Vec<(String, String)>);

fn trace(source: &str) -> Vec<Entry> {
    let mut interpreter = Interpreter::default();
    interpreter.trace = Some(Default::default());
    interpreter.run(&parse(source).unwrap()).unwrap();
    let trace = interpreter.finish_trace().unwrap();
    trace
        .rows
        .into_iter()
        .map(|r| (r.line, r.place, r.call, r.names))
        .collect()
}

fn names(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn records_kore_sore_and_locals_at_their_lines() {
    let source = "【分散】とは 説\n  仮【中心】は 総足 割 個\n  引 【中心】 自掛 総足 割 個。\n80 65 92 71 【分散】";
    assert_eq!(
        trace(source),
        vec![
            (1, "【分散】".into(), 1, names(&[("これ", "80 65 92 71")])),
            (2, "【分散】".into(), 1, names(&[("仮【中心】", "77")])),
        ]
    );
    assert_eq!(
        trace("【差】とは これ 引 それ\n10 【差】 3"),
        vec![(
            1,
            "【差】".into(),
            1,
            names(&[("これ", "10"), ("それ", "3")])
        )]
    );
}

#[test]
fn records_are_and_koko_inside_anonymous_operations() {
    let rows = trace("10 20 各（引 （あれ 総足））");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].1, "（引 （あれ 総足））");
    assert_eq!(
        rows[1].3,
        names(&[("これ", "20"), ("あれ", "10 20"), ("ここ", "2")])
    );
}

#[test]
fn keeps_only_the_first_calls_and_counts_the_rest() {
    let mut interpreter = Interpreter::default();
    interpreter.trace = Some(Default::default());
    interpreter
        .run(&parse("100 連 各（足 1） 個").unwrap())
        .unwrap();
    let trace = interpreter.finish_trace().unwrap();
    assert_eq!(trace.rows.len(), 3);
    assert_eq!(trace.calls[0].calls, 100);
}

#[test]
fn nothing_is_recorded_unless_asked() {
    let mut interpreter = Interpreter::default();
    interpreter
        .run(&parse("【f】とは これ 足 1\n1 【f】").unwrap())
        .unwrap();
    assert!(interpreter.finish_trace().is_none());
}

fn strip_marks(text: &str) -> String {
    let mut out = String::new();
    let mut in_key = false;
    for ch in text.chars() {
        match ch {
            '\u{E000}' => in_key = true,
            '\u{E001}' => in_key = false,
            '\u{E002}' => {}
            _ if in_key => {}
            _ => out.push(ch),
        }
    }
    out
}

#[test]
fn marked_readings_match_plain_readings_for_every_sample() {
    use azaori::readings::{read_program, read_program_marked};
    for entry in std::fs::read_dir("samples").unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        let program = parse(&source).unwrap();
        let plain = read_program(&program);
        let marked: Vec<_> = read_program_marked(&program)
            .into_iter()
            .map(|(line, text)| (line, strip_marks(&text)))
            .collect();
        assert_eq!(plain, marked, "{}", path.display());
    }
}

#[test]
fn values_are_recorded_per_line_for_hovering() {
    let mut interpreter = Interpreter::default();
    interpreter.trace = Some(Default::default());
    let source = "【点数】は 80 65 92 71\n【平均】とは 総足 割 個\n【点数】 【平均】\n【分散】とは 説\n  仮【中心】は 総足 割 個\n  引 【中心】 自掛 総足 割 個。\n【点数】 【分散】";
    interpreter.run(&parse(source).unwrap()).unwrap();
    let trace = interpreter.finish_trace().unwrap();
    let find = |line: usize, key: &str| {
        trace
            .values
            .iter()
            .find(|v| v.line == line && v.key == key)
            .map(|v| v.values.clone())
    };
    assert_eq!(find(1, "【点数】"), Some(vec!["80 65 92 71".to_string()]));
    assert_eq!(find(2, "これ"), Some(vec!["80 65 92 71".to_string()]));
    assert_eq!(find(3, "【点数】"), Some(vec!["80 65 92 71".to_string()]));
    assert_eq!(find(5, "【中心】"), Some(vec!["77".to_string()]));
    assert_eq!(find(6, "【中心】"), Some(vec!["77".to_string()]));
    assert_eq!(find(6, "これ"), Some(vec!["80 65 92 71".to_string()]));
}

#[test]
fn step_values_and_the_values_before_an_error() {
    let mut interpreter = Interpreter::default();
    interpreter.trace = Some(Default::default());
    let source = "【点数】は 80 65 92 71\n【点数】 総足 割 （【点数】 個）\n【点数】 引 70 根 総足";
    let error = interpreter.run(&parse(source).unwrap()).unwrap_err();
    assert_eq!((error.line, error.col), (Some(3), Some(10)));
    let trace = interpreter.finish_trace().unwrap();
    let find = |line: usize, key: &str| {
        trace
            .values
            .iter()
            .find(|v| v.line == line && v.key == key)
            .map(|v| v.values.clone())
    };
    // 2行: 【点数】 総足 → 308、割 （…） → 77、（【点数】 個） → 4
    assert_eq!(find(2, "#5"), Some(vec!["308".to_string()]));
    assert_eq!(find(2, "#8"), Some(vec!["77".to_string()]));
    assert_eq!(find(2, "#16"), Some(vec!["4".to_string()]));
    // 3行: 引 70 まではできて、根（10列）でエラー。根と総足の結果はない
    assert_eq!(find(3, "#5"), Some(vec!["10 -5 22 1".to_string()]));
    assert_eq!(find(3, "#10"), None);
    assert_eq!(find(3, "#12"), None);
}
