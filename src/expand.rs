use crate::ast::{fn_label, Node};
use crate::parser::precedence;
use num_bigint::BigInt;
use serde_json::Value;
use std::collections::HashSet;

pub fn code_num(text: &str) -> String {
    if !text.contains('.') && !text.contains(['e', 'E']) {
        return text
            .parse::<BigInt>()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| text.into());
    }
    let Ok(n) = text.parse::<f64>() else {
        return text.into();
    };
    let mut result = n.to_string();
    if n.is_finite() && n.fract() == 0.0 && (1e15..1e16).contains(&n.abs()) {
        result.push_str(".0");
    }
    if n == 0.0 {
        return "0".into();
    }
    result
}

pub fn expand_program(program: &Node) -> Vec<(usize, String)> {
    let used = names(program);
    let mut count = 0;
    let mut next = || loop {
        count += 1;
        let name = format!("主語{count}");
        if !used.contains(&name) {
            break name;
        }
    };
    match program {
        Node::Program { body } => body
            .iter()
            .map(|s| (s.line(), expand_stmt(s, &mut next)))
            .collect(),
        _ => Vec::new(),
    }
}

fn expand_stmt(stmt: &Node, next: &mut dyn FnMut() -> String) -> String {
    match stmt {
        Node::Assign { name, expr, .. } => {
            let (before, text) = expand_top(expr, next);
            format!("{before}【{name}】は {text}")
        }
        Node::Define { name, train, .. } => match train.as_ref() {
            Node::Block { lines } => {
                let mut out = vec![format!("【{name}】とは 説")];
                for line in lines {
                    match line {
                        Node::LocalAssign { name, expr, .. } => {
                            out.push(format!("  仮【{name}】は {}", expand_expr(expr)))
                        }
                        Node::Result { expr, .. } => out.push(format!("  {}", expand_expr(expr))),
                        _ => {}
                    }
                }
                out.join("\n") + "。"
            }
            Node::Train { steps } => {
                format!("【{name}】とは {}", expand_steps(steps, Some("これ")))
            }
            _ => format!("【{name}】とは {}", expand_expr(train)),
        },
        Node::Show { expr, .. } => {
            let (before, text) = expand_top(expr, next);
            before + &text
        }
        Node::EndLine { .. } => "終".into(),
        _ => String::new(),
    }
}

fn expand_top(expr: &Node, next: &mut dyn FnMut() -> String) -> (String, String) {
    if let Node::Expr { subject, steps } = expr {
        if uses_subject(steps) && !is_plain(subject) {
            let name = next();
            let subject_name = format!("【{name}】");
            return (
                format!("【{name}】は {}\n", value(subject)),
                join(&subject_name, &expand_steps(steps, Some(&subject_name))),
            );
        }
    }
    (String::new(), expand_expr(expr))
}

/// （）の中など、先に名前を付けられない場所の式。主語がくり返し計算しても同じ値になるなら、
/// 主語をもう一度書いて省略を書き出す（乱・待 や自分で作った操作を含む主語は、省略のままにする）
pub fn expand_expr(expr: &Node) -> String {
    if let Node::Expr { subject, steps } = expr {
        let subject_text = value(subject);
        join(
            &subject_text,
            &expand_steps(
                steps,
                if is_plain(subject) || is_repeatable(subject) {
                    Some(&subject_text)
                } else {
                    None
                },
            ),
        )
    } else {
        String::new()
    }
}

fn join(subject: &str, steps: &str) -> String {
    if steps.is_empty() {
        subject.into()
    } else {
        format!("{subject} {steps}")
    }
}

pub fn expand_steps(steps: &[Node], subject: Option<&str>) -> String {
    let mut parts = Vec::new();
    for step in steps {
        let text = match step {
            Node::Step {
                function,
                right: None,
            } => fn_text(function),
            Node::Step {
                function,
                right: Some(right),
            } => {
                let right = if matches!(right.as_ref(), Node::Phrase { .. }) {
                    phrase_with_subject(right, subject)
                } else {
                    value(right)
                };
                format!("{} {right}", fn_text(function))
            }
            Node::Loop {
                count, body, until, ..
            } => {
                let mut text = format!(
                    "回{} {}",
                    count
                        .as_ref()
                        .map(|n| format!(" {}", value(n)))
                        .unwrap_or_default(),
                    phrase_text(body)
                );
                if let Some(until) = until {
                    text.push_str(&format!(" 止 {}", phrase_text(until)));
                }
                text
            }
            Node::Branch {
                if_true, if_false, ..
            } => format!(
                "真 {} 偽 {}",
                side(if_true, subject),
                side(if_false, subject)
            ),
            Node::Pause {} => "停".into(),
            Node::End {} => "終".into(),
            Node::MathStep { expr } => math_text(expr, 0)
                .strip_prefix('…')
                .unwrap_or_default()
                .trim()
                .to_owned(),
            _ => continue,
        };
        parts.push(text);
    }
    parts.join(" ")
}

fn phrase_with_subject(phrase: &Node, subject: Option<&str>) -> String {
    match subject {
        Some(s) => format!("（{s} {}）", phrase_text(phrase)),
        None => phrase_text(phrase),
    }
}

fn side(node: &Node, subject: Option<&str>) -> String {
    match node {
        Node::End {} => "終".into(),
        Node::Phrase { .. } => phrase_with_subject(node, subject),
        _ => value(node),
    }
}

pub fn fn_text(function: &Node) -> String {
    match function {
        Node::AnonFn { body, .. } => match body.as_ref() {
            Node::Train { steps } => format!("（{}）", expand_steps(steps, Some("これ"))),
            _ => format!("（{}）", expand_expr(body)),
        },
        Node::DerivedFn { modifier, base, .. } => format!("{modifier}{}", fn_text(base)),
        _ => fn_label(function),
    }
}

pub fn anonymous_label(body: &Node) -> String {
    match body {
        Node::Train { steps } => format!("（{}）", expand_steps(steps, None)),
        _ => format!("（{}）", expand_expr(body)),
    }
}

fn phrase_text(phrase: &Node) -> String {
    match phrase {
        Node::Phrase { function, right } => {
            let text = fn_text(function);
            match right {
                Some(r) => format!("{text} {}", value(r)),
                None => text,
            }
        }
        _ => String::new(),
    }
}

pub fn value(node: &Node) -> String {
    match node {
        Node::Strand { values } => values
            .iter()
            .map(|s| code_num(s))
            .collect::<Vec<_>>()
            .join(" "),
        Node::StrLit { text, quote } => format!(
            "{}{text}{}",
            quote.chars().next().unwrap_or('「'),
            quote.chars().last().unwrap_or('」')
        ),
        Node::ItemList { items } => items.iter().map(value).collect::<Vec<_>>().join(" "),
        Node::NameRef { name, .. } => format!("【{name}】"),
        Node::KoreRef {} => "これ".into(),
        Node::SoreRef {} => "それ".into(),
        Node::AreRef {} => "あれ".into(),
        Node::KokoRef {} => "ここ".into(),
        Node::Group { expr } => format!("（{}）", expand_expr(expr)),
        Node::MathOp { .. } => math_text(node, 0),
        _ => "？".into(),
    }
}

fn math_text(node: &Node, outer: u8) -> String {
    if matches!(node, Node::Acc {}) {
        return "…".into();
    }
    let Node::MathOp { op, left, right } = node else {
        return value(node);
    };
    if op == "負" {
        return format!("-{}", math_text(right, 4));
    }
    let p = precedence(op);
    let text = format!(
        "{} {op} {}",
        left.as_ref().map(|l| math_text(l, p)).unwrap_or_default(),
        math_text(right, if op == "^" { p } else { p + 1 })
    );
    if p < outer {
        format!("（{text}）")
    } else {
        text
    }
}

fn is_plain(node: &Node) -> bool {
    match node {
        Node::Strand { .. }
        | Node::StrLit { .. }
        | Node::ItemList { .. }
        | Node::NameRef { .. }
        | Node::KoreRef {}
        | Node::SoreRef {}
        | Node::AreRef {}
        | Node::KokoRef {} => true,
        Node::MathOp { left, right, .. } => {
            left.as_ref().is_none_or(|l| is_plain(l)) && is_plain(right)
        }
        _ => false,
    }
}

/// もう一度計算しても同じ値になり、外の世界にも何もしない式か
fn is_repeatable(node: &Node) -> bool {
    fn visit(v: &Value) -> bool {
        match v {
            Value::Object(obj) => {
                let pure = match obj.get("type").and_then(Value::as_str) {
                    Some("NameFn" | "Pause" | "End") => false,
                    Some("PrimFn") => {
                        let name = obj.get("name").and_then(Value::as_str).unwrap_or("");
                        !matches!(
                            name,
                            "乱" | "抽" | "待" | "認" | "どれ" | "外" | "外読" | "外書" | "置"
                        )
                    }
                    _ => true,
                };
                pure && obj.values().all(visit)
            }
            Value::Array(items) => items.iter().all(visit),
            _ => true,
        }
    }
    serde_json::to_value(node).is_ok_and(|v| visit(&v))
}

fn uses_subject(steps: &[Node]) -> bool {
    steps.iter().any(|s| match s {
        Node::Step { right: Some(r), .. } => matches!(r.as_ref(), Node::Phrase { .. }),
        Node::Branch {
            if_true, if_false, ..
        } => {
            matches!(if_true.as_ref(), Node::Phrase { .. })
                || matches!(if_false.as_ref(), Node::Phrase { .. })
        }
        _ => false,
    })
}

fn names(node: &Node) -> HashSet<String> {
    fn visit(v: &Value, names: &mut HashSet<String>) {
        match v {
            Value::Object(obj) => {
                if matches!(
                    obj.get("type").and_then(Value::as_str),
                    Some("NameRef" | "NameFn" | "Assign" | "Define")
                ) {
                    if let Some(name) = obj.get("name").and_then(Value::as_str) {
                        names.insert(name.into());
                    }
                }
                for v in obj.values() {
                    visit(v, names);
                }
            }
            Value::Array(items) => {
                for v in items {
                    visit(v, names);
                }
            }
            _ => {}
        }
    }
    let mut result = HashSet::new();
    visit(
        &serde_json::to_value(node).expect("serializable syntax tree"),
        &mut result,
    );
    result
}
