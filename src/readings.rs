use crate::ast::{fn_label, Node};
use crate::expand::code_num;
use crate::parser::{language, precedence};

#[derive(Default)]
struct Reader {
    args: Vec<(String, String, String)>,
    subjects: Vec<String>,
    /// 名前（点数・これ など）に目印を付けるか（プレイグラウンドで中身を見せるため）
    marked: bool,
}

/// 目印を取り除く（目印を付けても、文の判定が変わらないようにするため）。
pub fn strip_marks(text: &str) -> String {
    let mut out = String::new();
    let mut in_key = false;
    for ch in text.chars() {
        match ch {
            MARK_START => in_key = true,
            MARK_KEY => in_key = false,
            MARK_END => {}
            _ if in_key => {}
            _ => out.push(ch),
        }
    }
    out
}

/// 目印: 「MARK_START 鍵 MARK_KEY 見せる字 MARK_END」。
/// 鍵は 【点数】・これ・それ・あれ・ここ、または途中のまとまり（手順）の「#列」。目印は入れ子になる。
pub const MARK_START: char = '\u{E000}';
pub const MARK_KEY: char = '\u{E001}';
pub const MARK_END: char = '\u{E002}';

pub fn read_program(program: &Node) -> Vec<(usize, String)> {
    read_program_with(program, false)
}

/// 名前に目印を付けた書き下し（プレイグラウンド用）。目印を除くと read_program と同じ文になる。
pub fn read_program_marked(program: &Node) -> Vec<(usize, String)> {
    read_program_with(program, true)
}

fn read_program_with(program: &Node, marked: bool) -> Vec<(usize, String)> {
    let mut reader = Reader {
        marked,
        ..Reader::default()
    };
    let mut result = Vec::new();
    if let Node::Program { body } = program {
        for stmt in body {
            result.push((stmt.line(), reader.stmt(stmt)));
            if let Node::Define { train, .. } = stmt {
                if let Node::Block { lines } = train.as_ref() {
                    for (i, stmt) in lines.iter().enumerate() {
                        let text = match stmt {
                            Node::LocalAssign { name, expr, .. } => {
                                let shown = reader.mark(&format!("【{name}】"), name);
                                format!("（仮に）{shown}は、{}", reader.expr(expr))
                            }
                            Node::Result { expr, .. } if i == lines.len() - 1 => {
                                format!("結果は、{}", reader.expr(expr))
                            }
                            Node::Result { expr, .. } => reader.expr(expr),
                            _ => continue,
                        };
                        result.push((stmt.line(), format!("　{text}")));
                    }
                }
            }
        }
    }
    result
}

fn comparisons(label: &str) -> bool {
    matches!(label, "等" | "異" | "超" | "未満" | "以上" | "以下" | "含")
}
fn logic(label: &str) -> bool {
    matches!(label, "且" | "又")
}

fn fill(template: &str, s: &str, r: &str) -> String {
    let mut output = String::new();
    let mut rest = template;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("{S}") {
            output.push_str(s);
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("{R}") {
            output.push_str(r);
            rest = tail;
        } else {
            let ch = rest.chars().next().unwrap();
            output.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    output
}

fn label(node: &Node) -> String {
    match node {
        Node::PrimFn { name, .. } | Node::NameFn { name, .. } => name.clone(),
        Node::DerivedFn { modifier, base, .. } => format!("{modifier}{}", label(base)),
        Node::InnerFn {
            reduce, combine, ..
        } => format!("内{}{}", label(reduce), label(combine)),
        Node::AnonFn { .. } => fn_label(node),
        _ => String::new(),
    }
}

fn noun(node: &Node) -> String {
    if let Node::PrimFn { name, .. } = node {
        if let Some(noun) = language()["primitives"][name]["noun"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            return noun.into();
        }
    }
    label(node)
}

fn axis(modifier: &str) -> Option<&'static str> {
    match modifier {
        "前" => Some("縦（前の向き）"),
        "中" => Some("中の向き"),
        "後" => Some("横（後の向き）"),
        _ => None,
    }
}

fn unwrap(text: &str) -> String {
    if !text.starts_with('（') || !text.ends_with('）') {
        return text.into();
    }
    let mut depth = 0;
    let count = text.chars().count();
    for (i, ch) in text.chars().enumerate() {
        depth += match ch {
            '（' => 1,
            '）' => -1,
            _ => 0,
        };
        if depth == 0 && i < count - 1 {
            return text.into();
        }
    }
    text.chars().skip(1).take(count - 2).collect()
}

impl Reader {
    fn mark(&self, key: &str, text: &str) -> String {
        if self.marked {
            format!("{MARK_START}{key}{MARK_KEY}{text}{MARK_END}")
        } else {
            text.into()
        }
    }

    fn stmt(&mut self, stmt: &Node) -> String {
        match stmt {
            Node::Assign { name, expr, .. } => {
                let shown = self.mark(&format!("【{name}】"), name);
                format!("{shown}は、{}", self.expr(expr))
            }
            Node::Define { name, train, .. } => match train.as_ref() {
                Node::Block { .. } => {
                    format!("{name}とは、次の手順（「。」の前の最後の手順が結果）")
                }
                Node::Train { steps } => {
                    let kore = self.mark("これ", "これ");
                    format!("{name}とは、{}", self.steps(steps, &kore))
                }
                _ => format!("{name}とは、{}", self.expr(train)),
            },
            Node::Show { expr, .. } => self.expr(expr),
            Node::EndLine { .. } => "ここでプログラムを終える".into(),
            _ => String::new(),
        }
    }
    fn expr(&mut self, expr: &Node) -> String {
        if let Node::Expr { subject, steps } = expr {
            let subject = self.value(subject);
            self.steps(steps, &subject)
        } else {
            String::new()
        }
    }
    fn steps(&mut self, steps: &[Node], subject: &str) -> String {
        self.subjects.push(subject.into());
        let result = self.steps_inner(steps, subject);
        self.subjects.pop();
        result
    }
    fn steps_inner(&mut self, steps: &[Node], subject: &str) -> String {
        let mut text = subject.to_owned();
        let mut is_condition = false;
        for step in steps {
            text = self.step_text(step, text, subject, &mut is_condition);
            if let Some(key) = step.step_key() {
                // 途中のまとまり: この手順までの読みを囲む（マウスを乗せると、その手順の結果が見える）
                text = self.mark(&key, &text);
            }
        }
        text
    }

    fn step_text(
        &mut self,
        step: &Node,
        text: String,
        subject: &str,
        is_condition: &mut bool,
    ) -> String {
        let mut text = text;
        {
            match step {
                Node::Loop {
                    count, body, until, ..
                } => {
                    let body_text = self.phrase_label(body);
                    text = if let Some(until) = until {
                        let until = self.phrase(until, "その値");
                        let limit = count
                            .as_ref()
                            .map(|n| format!("最大{}回、", self.value(n)))
                            .unwrap_or_default();
                        format!("{text}に「{body_text}」を、{limit}「{until}」となるまでくりかえしたもの")
                    } else {
                        let count = count.as_ref().map(|n| self.value(n)).unwrap_or_default();
                        format!("{text}に「{body_text}」を{count}回くりかえしたもの")
                    };
                    *is_condition = false;
                }
                Node::Branch {
                    if_true, if_false, ..
                } => {
                    text = format!(
                        "「{text}」が真なら{}、偽なら{}",
                        self.side(if_true, subject),
                        self.side(if_false, subject)
                    );
                    *is_condition = false;
                }
                Node::Pause {} => text.push_str("（ここで途中の値を表示）"),
                Node::End {} => text.push_str("（ここでプログラムを終える）"),
                Node::MathStep { expr } => {
                    text = format!("（{}）", self.math(expr, 0, &text));
                    *is_condition = false;
                }
                Node::Step { function, right } => {
                    let name = label(function);
                    if let Some(right) = right {
                        let right = if matches!(right.as_ref(), Node::Phrase { .. }) {
                            self.phrase(right, subject)
                        } else {
                            self.value(right)
                        };
                        let left = if *is_condition && !logic(&name) {
                            format!("（{text}）")
                        } else {
                            text
                        };
                        text = self.dyad(function, &left, &right);
                    } else {
                        let condition_template = if *is_condition {
                            match name.as_str() {
                                "総足" => Some("{S}ものの個数"),
                                "最小" => Some("{S}かどうか（すべて）"),
                                "最大" => Some("{S}かどうか（どれか1つでも）"),
                                "検" => Some("「{S}」ことを確かめたもの"),
                                "否" => Some("「{S}」ではない"),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        if let Some(template) = condition_template {
                            if name == "総足"
                                && ["以上", "以下", "未満"]
                                    .iter()
                                    .any(|s| strip_marks(&text).ends_with(s))
                            {
                                text.push('の');
                            }
                            text = fill(template, &text, "");
                        } else {
                            let left = if *is_condition {
                                format!("（{text}）")
                            } else {
                                text
                            };
                            text = self.monad(function, &left);
                        }
                    }
                    *is_condition = (right.is_some() && (comparisons(&name) || logic(&name)))
                        || (right.is_none() && name == "否");
                }
                _ => {}
            }
        }
        text
    }
    fn side(&mut self, node: &Node, subject: &str) -> String {
        match node {
            Node::End {} => "プログラムを終える".into(),
            Node::Phrase { .. } => self.phrase(node, subject),
            _ => self.value(node),
        }
    }
    fn phrase_label(&mut self, phrase: &Node) -> String {
        if let Node::Phrase { function, right } = phrase {
            let label = label(function);
            match right {
                Some(r) => format!("{label} {}", self.value(r)),
                None => label,
            }
        } else {
            String::new()
        }
    }
    fn phrase(&mut self, phrase: &Node, subject: &str) -> String {
        if let Node::Phrase { function, right } = phrase {
            match right {
                Some(r) => {
                    let r = self.value(r);
                    self.dyad(function, subject, &r)
                }
                None => self.monad(function, subject),
            }
        } else {
            String::new()
        }
    }
    fn value(&mut self, node: &Node) -> String {
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
            Node::ItemList { items } => items
                .iter()
                .map(|n| self.value(n))
                .collect::<Vec<_>>()
                .join("・"),
            Node::NameRef { name, .. } => self.mark(&format!("【{name}】"), name),
            Node::KoreRef {} => self
                .args
                .last()
                .map(|a| a.0.clone())
                .unwrap_or_else(|| self.mark("これ", "これ")),
            Node::SoreRef {} => self
                .args
                .last()
                .map(|a| a.1.clone())
                .unwrap_or_else(|| self.mark("それ", "それ")),
            Node::AreRef {} => self
                .args
                .last()
                .map(|a| a.2.clone())
                .unwrap_or_else(|| self.mark("あれ", "あれ")),
            Node::KokoRef {} => self.mark("ここ", "今の番号"),
            Node::Group { expr } => {
                let wrap = matches!(expr.as_ref(), Node::Expr { steps, .. } if steps.len() > 1);
                let text = self.expr(expr);
                if wrap {
                    format!("（{text}）")
                } else {
                    text
                }
            }
            Node::MathOp { .. } => format!("（{}）", self.math(node, 0, "")),
            _ => "？".into(),
        }
    }
    fn math(&mut self, node: &Node, outer: u8, acc: &str) -> String {
        if matches!(node, Node::Acc {}) {
            return acc.into();
        }
        let Node::MathOp { op, left, right } = node else {
            return self.value(node);
        };
        if op == "負" {
            return format!("-{}", self.math(right, 4, acc));
        }
        let p = precedence(op);
        let left = left
            .as_ref()
            .map(|n| self.math(n, p, acc))
            .unwrap_or_default();
        let right = self.math(right, if op == "^" { p } else { p + 1 }, acc);
        let text = format!("{left} {op} {right}");
        if p < outer {
            format!("（{text}）")
        } else {
            text
        }
    }
    fn monad(&mut self, function: &Node, s: &str) -> String {
        match function {
            Node::PrimFn { name, .. } => fill(
                language()["primitives"][name]["reading1"]
                    .as_str()
                    .unwrap_or("{S}"),
                s,
                "",
            ),
            Node::DerivedFn { modifier, base, .. } => {
                if modifier == "自" {
                    if let Node::DerivedFn {
                        modifier: inner,
                        base,
                        ..
                    } = base.as_ref()
                    {
                        if inner == "表" {
                            return format!("{s}どうしの{}の表", noun(base));
                        }
                    }
                }
                if modifier == "各" {
                    return self.monad(base, &format!("{s}のそれぞれ"));
                }
                if let Some(axis) = axis(modifier) {
                    return format!("{}（{axis}）", self.monad(base, s));
                }
                if modifier == "隣" {
                    return format!("{s}の隣どうしの{}", noun(base));
                }
                if modifier == "非" {
                    return format!("「{}」ではない", self.monad(base, s));
                }
                let name = format!("{modifier}{}", label(base));
                let template = match name.as_str() {
                    "総足" => "{S}の合計",
                    "累足" => "{S}の累計",
                    "総掛" => "{S}をすべて掛けたもの",
                    "累掛" => "{S}を順に掛けていった途中経過",
                    "自掛" => "{S}の2乗",
                    "自足" => "{S}の2倍",
                    "総結" => "{S}をすべてつなげたもの",
                    _ => return format!("{s}の{name}"),
                };
                fill(template, s, "")
            }
            Node::NameFn { name, .. } => format!("{s}の{name}"),
            Node::AnonFn { body, .. } => self.anon(body, s, "それ"),
            _ => s.into(),
        }
    }
    fn dyad(&mut self, function: &Node, s: &str, r: &str) -> String {
        match function {
            Node::PrimFn { name, .. } => {
                let parts: Vec<&str> = r.split_whitespace().collect();
                match name.as_str() {
                    "番" if {
                        let n = r.replace(' ', "");
                        n.is_empty() || !n.chars().all(char::is_numeric)
                    } =>
                    {
                        return format!("{s}を「{}」の順に取り出したもの", unwrap(r))
                    }
                    "換" if split_items(r).len().is_multiple_of(2) => {
                        let parts = split_items(r);
                        let pairs = parts
                            .chunks_exact(2)
                            .map(|p| format!("{}を{}に", p[0], p[1]))
                            .collect::<Vec<_>>()
                            .join("、");
                        return format!("{s}の{pairs}換えたもの");
                    }
                    "位" if parts.len() > 1 => {
                        return format!("{s}を「{r}」の進み方で位取りしたもの")
                    }
                    "拾" if parts.len() == 2 => {
                        return format!("{s}の{}行{}列目", parts[0], parts[1])
                    }
                    "替" if parts.len() == 2 => {
                        return format!("{s}の{}番目を{}に替えたもの", parts[0], parts[1])
                    }
                    "替" if parts.len() == 3 => {
                        return format!(
                            "{s}の{}行{}列目を{}に替えたもの",
                            parts[0], parts[1], parts[2]
                        )
                    }
                    _ => {}
                }
                let template = language()["primitives"][name]["reading2"]
                    .as_str()
                    .unwrap_or("{S}と{R}");
                fill(
                    template,
                    s,
                    &if template.contains("「{R}」") {
                        unwrap(r)
                    } else {
                        r.into()
                    },
                )
            }
            Node::DerivedFn { modifier, base, .. } => {
                if let Some(axis) = axis(modifier) {
                    return format!("{}（{axis}）", self.dyad(base, s, r));
                }
                match modifier.as_str() {
                    "表" => format!("{s}と{r}の{}の表", noun(base)),
                    "各" => self.dyad(base, &format!("{s}のそれぞれ"), r),
                    "隣" => format!("{s}の{r}つずつの{}", noun(base)),
                    "毎" => format!("{s}に「{}」を{r}の1つずつでかけたもの", label(base)),
                    "試" => format!("{}（できなければ{r}）", self.monad(base, s)),
                    "反" => self.dyad(base, r, s),
                    "非" => format!("「{}」ではない", self.dyad(base, s, r)),
                    _ => format!("{s}と{r}"),
                }
            }
            Node::InnerFn {
                reduce, combine, ..
            } => format!("{s}と{r}の、{}の{}（内積）", noun(combine), noun(reduce)),
            Node::NameFn { name, .. } => format!("{s}と{r}の{name}"),
            Node::AnonFn { body, .. } => self.anon(body, s, r),
            _ => format!("{s}と{r}"),
        }
    }
    fn anon(&mut self, body: &Node, s: &str, r: &str) -> String {
        let are = self
            .subjects
            .last()
            .cloned()
            .unwrap_or_else(|| "あれ".into());
        self.args.push((s.into(), r.into(), are));
        let result = match body {
            Node::Train { steps } => self.steps(steps, s),
            _ => self.expr(body),
        };
        self.args.pop();
        result
    }
}

/// 「a」・「b」 のように ・ でつないだ読みを分ける（「」や『』の中の ・ は文字なので分けない）
fn split_items(text: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let (mut depth, mut start) = (0usize, 0);
    for (i, ch) in text.char_indices() {
        match ch {
            '「' | '『' => depth += 1,
            '」' | '』' => depth = depth.saturating_sub(1),
            '・' if depth == 0 => {
                items.push(&text[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    items.push(&text[start..]);
    items
}
