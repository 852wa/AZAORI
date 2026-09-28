use crate::ast::{fn_label, Node};
use crate::error::{AzaError, AzaResult};
use crate::lexer::{tokenize, Token};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

pub fn language() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../conformance/language.json"))
            .expect("valid language metadata")
    })
}

/// （）の入れ子の上限。深すぎる式で処理系のスタックがあふれないようにする
pub const MAX_NEST: usize = 100;
/// 1つの文（説の中では1つの手順）で使える記号の計算（+ - × ÷ ^ と符号）の数
pub const MAX_MATH_OPS: usize = 1000;
/// 続けて書いた漢字（熟語）の長さの上限。熟語の前の字を何重にも重ねた式を作らせない
pub const MAX_WORD: usize = 30;

/// 記号の優先順位（conformance/language.json の math_precedence）。記号でなければ 0
pub fn precedence(op: &str) -> u8 {
    language()["math_precedence"][op]
        .as_u64()
        .map_or(0, |p| p as u8)
}

/// 記号の計算にあたる字（conformance/language.json の math_ops）。符号の - は 引（0 から引く）
pub fn math_name(op: &str) -> Option<&'static str> {
    language()["math_ops"][if op == "負" { "-" } else { op }].as_str()
}

fn control(word: &str) -> bool {
    matches!(word, "回" | "止" | "真" | "偽" | "停" | "終")
}
/// 熟語の前の字か（一覧は conformance/language.json の modifiers。ここに字を書き写さない）
fn modifier(ch: char) -> bool {
    language()["modifiers"].get(ch.to_string()).is_some()
}
/// 熟語の前の字の決まり（base: うしろの演算がとる値の数、arity: できた熟語がとる値の数）
fn modifier_rule(ch: char, key: &str) -> Option<&'static str> {
    language()["modifier_rules"]["rules"][ch.to_string()][key].as_str()
}
fn all_modifiers(word: &str) -> bool {
    !word.is_empty() && word.chars().all(modifier)
}

fn mark(mut node: Node, first: &Token, last: &Token) -> Node {
    match &mut node {
        Node::NameRef { col, end, .. }
        | Node::PrimFn { col, end, .. }
        | Node::NameFn { col, end, .. }
        | Node::DerivedFn { col, end, .. }
        | Node::InnerFn { col, end, .. }
        | Node::AnonFn { col, end, .. }
        | Node::Loop { col, end, .. }
        | Node::Branch { col, end, .. } => {
            *col = first.col as i64;
            *end = last.end as i64;
        }
        _ => {}
    }
    node
}

fn prim(name: &str) -> Node {
    Node::PrimFn {
        name: name.to_owned(),
        col: -1,
        end: -1,
    }
}
fn named(name: &str) -> Node {
    Node::NameFn {
        name: name.to_owned(),
        col: -1,
        end: -1,
    }
}

pub fn parse(source: &str) -> AzaResult<Node> {
    parse_with_elisions(source).map(|(program, _)| program)
}

/// 書かれていない値を補って読む場所。エディタはここに薄い字で `text` を重ねて見せる。
/// 主語の省略（右の演算に主語をかける）と、説の中で演算から始まる手順（これ から読む）の2つ。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Elision {
    pub line: usize,
    /// 補う値を重ねる字（演算）の始まりと終わり（元の行の文字で数える）
    pub col: usize,
    pub end: usize,
    /// 補って読む値（主語の書き方、または これ）
    pub text: String,
}

pub fn parse_with_elisions(source: &str) -> AzaResult<(Node, Vec<Elision>)> {
    let mut parser = Parser {
        tokens: tokenize(source)?,
        pos: 0,
        fn_names: HashMap::new(),
        in_definition: false,
        anon_depth: 0,
        lines: crate::lexer::source_lines(source)
            .iter()
            .map(|line| line.chars().collect())
            .collect(),
        subjects: Vec::new(),
        elisions: Vec::new(),
        nest: 0,
        math_count: 0,
    };
    let program = parser.program()?;
    Ok((program, parser.elisions))
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    fn_names: HashMap<String, (bool, bool)>,
    in_definition: bool,
    anon_depth: usize,
    lines: Vec<Vec<char>>,
    /// 今読んでいる式の主語の書き方（省略のときに補う値）
    subjects: Vec<String>,
    elisions: Vec<Elision>,
    /// 今の（）の入れ子の深さ
    nest: usize,
    /// 今の文の記号の計算の数
    math_count: usize,
}

impl Parser {
    /// tokens[first..=last]（同じ行）の元の書き方
    fn source_text(&self, first: usize, last: usize) -> String {
        let (a, b) = (&self.tokens[first], &self.tokens[last]);
        match self.lines.get(a.line.wrapping_sub(1)) {
            Some(line) if a.line == b.line && a.col <= b.end && b.end <= line.len() => {
                line[a.col..b.end].iter().collect()
            }
            _ => a.value.clone(),
        }
    }
    fn with_subject<T>(&mut self, subject: String, f: impl FnOnce(&mut Self) -> T) -> T {
        self.subjects.push(subject);
        let result = f(self);
        self.subjects.pop();
        result
    }
    fn peek(&self, offset: usize) -> &Token {
        &self.tokens[(self.pos + offset).min(self.tokens.len() - 1)]
    }
    fn advance(&mut self) -> Token {
        let token = self.peek(0).clone();
        self.pos += 1;
        token
    }
    fn error(&self, message: impl Into<String>) -> AzaError {
        self.error_at(message, self.peek(0))
    }
    fn error_at(&self, message: impl Into<String>, token: &Token) -> AzaError {
        let mut error = AzaError::new(message)
            .at(token.line)
            .place(token.col, token.end)
            .with_kind("syntax");
        if matches!(token.kind.as_str(), "NEWLINE" | "EOF") {
            error.end = Some(-1);
        }
        error
    }
    fn at_control(&self, word: &str) -> bool {
        self.peek(0).kind == "FN" && self.peek(0).value == word
    }
    fn at_sign(&self) -> bool {
        let (t, n) = (self.peek(0), self.peek(1));
        t.kind == "OP"
            && t.value == "-"
            && n.line == t.line
            && n.col == t.end
            && matches!(
                n.kind.as_str(),
                "NUM" | "NAME" | "LP" | "KORE" | "SORE" | "ARE" | "KOKO"
            )
    }
    fn at_atom(&self) -> bool {
        let t = self.peek(0);
        matches!(
            t.kind.as_str(),
            "NUM" | "STR" | "EXT" | "KORE" | "SORE" | "ARE" | "KOKO"
        ) || (t.kind == "LP" && !self.at_anon(self.pos))
            || (t.kind == "NAME" && !self.fn_names.contains_key(&t.value))
    }
    fn at_value(&self) -> bool {
        self.at_atom() || self.at_sign()
    }
    fn at_fn(&self) -> bool {
        let t = self.peek(0);
        (t.kind == "FN" && !control(&t.value))
            || (t.kind == "NAME" && self.fn_names.contains_key(&t.value))
            || self.at_anon(self.pos)
    }
    fn at_anon(&self, i: usize) -> bool {
        let t = &self.tokens[i];
        let n = &self.tokens[(i + 1).min(self.tokens.len() - 1)];
        if t.kind != "LP" {
            return false;
        }
        if (n.kind == "FN" && !control(&n.value))
            || (n.kind == "NAME" && self.fn_names.contains_key(&n.value))
        {
            return true;
        }
        i > 0 && {
            let before = &self.tokens[i - 1];
            before.kind == "FN"
                && all_modifiers(&before.value)
                && before.line == t.line
                && before.end == t.col
                && !matches!(n.kind.as_str(), "NEWLINE" | "EOF")
        }
    }
    fn line_end(&self, mut i: usize) -> usize {
        while !matches!(self.tokens[i].kind.as_str(), "NEWLINE" | "EOF") {
            i += 1;
        }
        i
    }
    /// 「説」から対になる「。」まで（「。」がなければファイルの終わりまで）
    fn block_end(&self) -> usize {
        let mut i = self.pos;
        while !matches!(self.tokens[i].kind.as_str(), "PERIOD" | "EOF") {
            i += 1;
        }
        i
    }
    fn uses_sore(&self, start: usize, end: usize) -> bool {
        let mut stack = Vec::new();
        for i in start..end {
            match self.tokens[i].kind.as_str() {
                "LP" => stack.push(self.at_anon(i)),
                "RP" => {
                    stack.pop();
                }
                "SORE" if !stack.iter().any(|v| *v) => return true,
                _ => {}
            }
        }
        false
    }
    fn program(&mut self) -> AzaResult<Node> {
        let mut body = Vec::new();
        while self.peek(0).kind != "EOF" {
            body.push(self.line()?);
            if self.peek(0).kind == "NEWLINE" {
                self.advance();
            } else if self.peek(0).kind != "EOF" {
                return Err(self.error(self.unexpected(self.peek(0))));
            }
        }
        Ok(Node::Program { body })
    }
    fn enter_nest(&mut self) -> AzaResult<()> {
        if self.nest >= MAX_NEST {
            return Err(self.error(format!("（）の入れ子が深すぎます（{MAX_NEST}段までです）")));
        }
        self.nest += 1;
        Ok(())
    }
    fn count_math_op(&mut self) -> AzaResult<()> {
        self.math_count += 1;
        if self.math_count > MAX_MATH_OPS {
            return Err(self.error(format!("1つの式の記号の計算が多すぎます（{MAX_MATH_OPS}個までです。途中で【名前】を付けて分けてください）")));
        }
        Ok(())
    }
    fn line(&mut self) -> AzaResult<Node> {
        self.math_count = 0;
        let (first, second) = (self.peek(0).clone(), self.peek(1).clone());
        if self.at_control("終") && matches!(second.kind.as_str(), "NEWLINE" | "EOF") {
            self.advance();
            return Ok(Node::EndLine { line: first.line });
        }
        if first.kind == "NAME" && second.kind == "WA" {
            self.advance();
            self.advance();
            let expr = Box::new(self.expr()?);
            self.fn_names.remove(&first.value);
            return Ok(Node::Assign {
                name: first.value,
                expr,
                line: first.line,
            });
        }
        if first.kind == "NAME" && second.kind == "TOWA" {
            return self.definition(first);
        }
        Ok(Node::Show {
            expr: Box::new(self.expr()?),
            line: first.line,
        })
    }
    fn definition(&mut self, name: Token) -> AzaResult<Node> {
        self.advance();
        self.advance();
        let block = self.at_control("説");
        let end = if block {
            self.block_end()
        } else {
            self.line_end(self.pos)
        };
        self.fn_names
            .entry(name.value.clone())
            .or_insert((true, false));
        let dyadic = self.uses_sore(self.pos, end);
        self.fn_names.insert(name.value.clone(), (!dyadic, dyadic));
        let outer = self.in_definition;
        self.in_definition = true;
        let result = (|| {
            if block {
                return self.block();
            }
            if self.at_value() {
                let expr = self.expr()?;
                if !mentions_arguments(&expr) {
                    return Err(self.error_at("「とは」のうしろは演算か、これ・それ・ここ から始めます（値に名前を付けるなら「は」を使います）", &name));
                }
                Ok(expr)
            } else {
                let steps = self.with_subject("これ".into(), |p| p.steps())?;
                if steps.is_empty() {
                    return Err(self.error(
                        "「とは」のうしろに演算を書いてください（例: 【平均】とは 総足 割 個）",
                    ));
                }
                Ok(Node::Train { steps })
            }
        })();
        self.in_definition = outer;
        Ok(Node::Define {
            name: name.value,
            train: Box::new(result?),
            line: name.line,
            dyadic,
        })
    }
    /// 【名前】とは 説 … 。 手順は改行か「、」で区切り、「。」で閉じる。字下げは見た目だけで意味を持たない。
    fn block(&mut self) -> AzaResult<Node> {
        let start = self.advance();
        let mut lines = Vec::new();
        let mut need_step = false;
        loop {
            match self.peek(0).kind.as_str() {
                "NEWLINE" => {
                    self.advance();
                    continue;
                }
                "COMMA" => {
                    if lines.is_empty() || need_step {
                        return Err(self.error("「、」の前に手順が必要です（「、」は「説」の手順と手順の間に書きます）"));
                    }
                    need_step = true;
                    self.advance();
                    continue;
                }
                "PERIOD" => {
                    if need_step {
                        return Err(self.error(
                            "「、」のうしろに手順が必要です（最後の手順は「。」で閉じます）",
                        ));
                    }
                    break;
                }
                "EOF" => return Err(self.unclosed_block(&start)),
                _ => {}
            }
            let (first, second) = (self.peek(0).clone(), self.peek(1).clone());
            if first.kind == "NAME"
                && matches!(second.kind.as_str(), "WA" | "TOWA")
                && !lines.is_empty()
                && !need_step
                && first.line != self.tokens[self.pos - 1].line
            {
                return Err(self.error_at(
                    format!("{}行目の「説」が「。」で閉じられていません（最後の手順のうしろに「。」を書きます）。「説」の中で名前を付けるなら 仮【{}】は … と書きます", start.line, first.value),
                    &start,
                ));
            }
            lines.push(self.block_line()?);
            need_step = false;
            match self.peek(0).kind.as_str() {
                "NEWLINE" | "COMMA" | "PERIOD" => {}
                "EOF" => return Err(self.unclosed_block(&start)),
                _ => return Err(self.error(self.unexpected(self.peek(0)))),
            }
        }
        let period = self.advance();
        if lines.is_empty() {
            return Err(self.error_at("「説」から「。」までの間に、操作の手順を書きます（例: 【幅】とは 説 仮【最高】は これ 最大、【最高】 引 （これ 最小）。）", &start));
        }
        if !matches!(self.peek(0).kind.as_str(), "NEWLINE" | "EOF") {
            return Err(self.error("「。」で「説」は終わります。「。」のうしろには何も書けません（次の行に書いてください）"));
        }
        if !matches!(lines.last(), Some(Node::Result { .. })) {
            return Err(self.error_at("「説」の最後の手順には、操作の結果になる式を書きます（仮【名前】は … で終われません）", &period));
        }
        Ok(Node::Block { lines })
    }
    fn unclosed_block(&self, start: &Token) -> AzaError {
        self.error_at(
            format!("{}行目の「説」が「。」で閉じられていません（最後の手順のうしろに「。」を書きます）", start.line),
            start,
        )
    }
    fn block_line(&mut self) -> AzaResult<Node> {
        self.math_count = 0;
        let (first, second, third) = (
            self.peek(0).clone(),
            self.peek(1).clone(),
            self.peek(2).clone(),
        );
        if self.at_control("仮") {
            if second.kind != "NAME" || third.kind != "WA" {
                return Err(self.error("「仮」のうしろには【名前】は を書きます（例: 仮【平均】は これ 総足 割 （これ 個））"));
            }
            if self.fn_names.contains_key(&second.value) {
                return Err(self.error_at(
                    format!(
                        "【{}】は操作の名前です。仮の名前には別の名前を付けてください",
                        second.value
                    ),
                    &second,
                ));
            }
            self.advance();
            self.advance();
            self.advance();
            return Ok(Node::LocalAssign {
                name: second.value,
                expr: Box::new(self.body_expr()?),
                line: first.line,
            });
        }
        if first.kind == "NAME" && matches!(second.kind.as_str(), "WA" | "TOWA") {
            return Err(self.error_at(format!("「説」の中で名前を付けるときは、仮【{}】は … と書きます（その操作の中だけで使う名前になります）", first.value), &first));
        }
        Ok(Node::Result {
            expr: Box::new(self.body_expr()?),
            line: first.line,
        })
    }
    fn body_expr(&mut self) -> AzaResult<Node> {
        if self.at_fn() {
            let t = self.peek(0).clone();
            self.elisions.push(Elision {
                line: t.line,
                col: t.col,
                end: t.end,
                text: "これ".into(),
            });
            Ok(Node::Expr {
                subject: Box::new(Node::KoreRef {}),
                steps: self.with_subject("これ".into(), |p| p.steps())?,
            })
        } else {
            self.expr()
        }
    }
    fn unexpected(&self, t: &Token) -> String {
        match t.kind.as_str() {
            "OP" => format!("「{}」の前に値が必要です（演算の字のすぐうしろに記号は書けません。（）でまとめてください）", t.value),
            "WA" | "TOWA" => format!("「{}」は行の最初の【名前】のすぐうしろにだけ書けます", t.value),
            "KORE" | "SORE" | "ARE" | "KOKO" => format!("「{}」の前に演算が必要です（値と値の間には演算が必要です）", t.value),
            "RP" => "（ に対応しない ）があります".into(),
            "PERIOD" => "「。」は「説」の終わりにだけ書きます（例: 【幅】とは 説 仮【最高】は これ 最大、【最高】 引 （これ 最小）。）".into(),
            "COMMA" => "「、」は「説」の中で、手順と手順の間に書きます".into(),
            _ => format!("ここに「{}」は書けません", t.value),
        }
    }
    fn expr(&mut self) -> AzaResult<Node> {
        if !self.at_value() {
            let t = self.peek(0).clone();
            let message = match t.kind.as_str() {
                "OP" => format!("「{}」の前に値が必要です", t.value),
                "NAME" => format!(
                    "【{0}】は操作です。値のうしろに置いて使います（例: 【点数】 【{0}】）",
                    t.value
                ),
                "FN" => {
                    self.build_fn(&t.value, &t)?;
                    format!(
                        "式は値から始めてください。AZAORI は左から右に読みます（例: 10 {}）",
                        t.value
                    )
                }
                _ => "ここには値（数、「文字」、【名前】、（まとまり））が必要です".into(),
            };
            return Err(self.error(message));
        }
        let first = self.pos;
        let subject = Box::new(self.value()?);
        let text = self.source_text(first, self.pos - 1);
        Ok(Node::Expr {
            subject,
            steps: self.with_subject(text, |p| p.steps())?,
        })
    }
    fn value(&mut self) -> AzaResult<Node> {
        self.math(1, None)
    }
    fn math(&mut self, min_prec: u8, left: Option<Node>) -> AzaResult<Node> {
        let mut left = match left {
            Some(value) => value,
            None => self.math_operand()?,
        };
        while self.peek(0).kind == "OP" && precedence(&self.peek(0).value) >= min_prec {
            self.count_math_op()?;
            let op = self.advance().value;
            let p = precedence(&op);
            let right = self.math(if op == "^" { p } else { p + 1 }, None)?;
            left = Node::MathOp {
                op,
                left: Some(Box::new(left)),
                right: Box::new(right),
            };
        }
        Ok(left)
    }
    fn math_operand(&mut self) -> AzaResult<Node> {
        if self.peek(0).kind == "OP" && self.peek(0).value == "-" {
            self.count_math_op()?;
            self.advance();
            return Ok(Node::MathOp {
                op: "負".into(),
                left: None,
                right: Box::new(self.math(3, None)?),
            });
        }
        if self.peek(0).kind == "OP" {
            return Err(self.error(format!("「{}」の前に値が必要です", self.peek(0).value)));
        }
        if !self.at_atom() {
            return Err(
                self.error("記号のうしろには値（数、【名前】、（まとまり）など）が必要です")
            );
        }
        self.atom()
    }
    fn atom(&mut self) -> AzaResult<Node> {
        let t = self.peek(0).clone();
        match t.kind.as_str() {
            "ARE" => {
                if self.anon_depth == 0 {
                    return Err(self.error("「あれ」は（）で作るその場の操作の中で、外側の主語を指します（例: 【点数】 各（引 （あれ 総足 割 （あれ 個））））"));
                }
                self.advance();
                Ok(Node::AreRef {})
            }
            "KOKO" => {
                if !self.in_definition && self.anon_depth == 0 {
                    return Err(self.error("「ここ」は 各・毎 でかける操作の中で、今の要素が何番目かを指します（例: 【氏名】 各（結 ここ））"));
                }
                self.advance();
                Ok(Node::KokoRef {})
            }
            "KORE" | "SORE" => {
                if !self.in_definition {
                    return Err(self.error(format!("「{}」は「とは」で操作を作るときに使います（例: 【距離】とは これ 引 それ 絶）", t.value)));
                }
                self.advance();
                Ok(if t.kind == "KORE" {
                    Node::KoreRef {}
                } else {
                    Node::SoreRef {}
                })
            }
            "NUM" | "STR" | "EXT" => {
                let mut tokens = Vec::new();
                while matches!(self.peek(0).kind.as_str(), "NUM" | "STR" | "EXT") {
                    tokens.push(self.advance());
                }
                if tokens.iter().all(|t| t.kind == "NUM") {
                    return Ok(Node::Strand {
                        values: tokens.into_iter().map(|t| t.value).collect(),
                    });
                }
                let mut items: Vec<Node> = tokens
                    .into_iter()
                    .map(|t| {
                        if t.kind == "NUM" {
                            Node::Strand {
                                values: vec![t.value],
                            }
                        } else {
                            Node::StrLit {
                                text: t.value,
                                quote: if t.kind == "EXT" {
                                    "\"\"".into()
                                } else if t.quote.is_empty() {
                                    "「」".into()
                                } else {
                                    t.quote
                                },
                            }
                        }
                    })
                    .collect();
                Ok(if items.len() == 1 {
                    items.remove(0)
                } else {
                    Node::ItemList { items }
                })
            }
            "NAME" => {
                self.advance();
                Ok(mark(
                    Node::NameRef {
                        name: t.value.clone(),
                        col: -1,
                        end: -1,
                    },
                    &t,
                    &t,
                ))
            }
            "LP" => {
                self.enter_nest()?;
                self.advance();
                let expr = Box::new(self.expr()?);
                if self.peek(0).kind != "RP" {
                    return Err(self.error("（ に対応する ）がありません"));
                }
                self.advance();
                self.nest -= 1;
                Ok(Node::Group { expr })
            }
            _ => Err(self.error("ここには値が必要です")),
        }
    }
    fn steps(&mut self) -> AzaResult<Vec<Node>> {
        let mut steps = Vec::new();
        loop {
            let t = self.peek(0).clone();
            if t.kind == "FN" && control(&t.value) {
                steps.push(self.control(&t)?);
                continue;
            }
            if t.kind == "OP" {
                steps.push(Node::MathStep {
                    expr: Box::new(self.math(1, Some(Node::Acc {}))?),
                });
                continue;
            }
            if !self.at_fn() {
                break;
            }
            let function = self.function()?;
            let (monad, dyad) = self.arity(&function);
            let label = fn_label(&function);
            let right = if self.at_value() {
                if !dyad {
                    return Err(self.error(format!(
                        "「{label}」は右に値をとりません（値と値の間には演算が必要です）"
                    )));
                }
                Some(Box::new(self.value()?))
            } else if self.at_fn() && dyad && !monad {
                let t = self.peek(0).clone();
                let phrase = self.phrase()?;
                self.elisions.push(Elision {
                    line: t.line,
                    col: t.col,
                    end: t.end,
                    text: self
                        .subjects
                        .last()
                        .cloned()
                        .unwrap_or_else(|| "これ".into()),
                });
                Some(Box::new(phrase))
            } else {
                if !monad {
                    return Err(self.error(format!("「{label}」の右に値が必要です")));
                }
                None
            };
            steps.push(Node::Step {
                function: Box::new(function),
                right,
            });
        }
        if self.at_value() {
            return Err(self
                .error("値のうしろに、演算なしで値が続いています。値と値の間には演算が必要です"));
        }
        Ok(steps)
    }
    fn control(&mut self, t: &Token) -> AzaResult<Node> {
        match t.value.as_str() {
            "回" => self.loop_(),
            "真" => self.branch(),
            "停" => {
                self.advance();
                Ok(Node::Pause {})
            }
            "終" => {
                self.advance();
                Ok(Node::End {})
            }
            "止" => {
                Err(self.error("「止」は「回」と組み合わせて使います（例: 1 回 自和 止 超 1000）"))
            }
            _ => Err(self.error(
                "「偽」は「真」と組み合わせて使います（例: 条件 真 「はい」 偽 「いいえ」）",
            )),
        }
    }
    fn loop_(&mut self) -> AzaResult<Node> {
        let start = self.advance();
        let count = if self.at_value() {
            Some(Box::new(self.value()?))
        } else {
            None
        };
        if !self.at_fn() {
            return Err(
                self.error("「回」のうしろには、くりかえす演算を書きます（例: 1 回 10 自和）")
            );
        }
        let body = Box::new(self.phrase()?);
        let until = if self.at_control("止") {
            self.advance();
            if !self.at_fn() {
                return Err(
                    self.error("「止」のうしろには、止まる条件を書きます（例: 止 超 1000）")
                );
            }
            Some(Box::new(self.phrase()?))
        } else {
            None
        };
        if count.is_none() && until.is_none() {
            return Err(self.error_at(
                "「回」には回数か「止」の条件が必要です（例: 回 10 自足 ／ 回 自足 止 超 1000）",
                &start,
            ));
        }
        Ok(mark(
            Node::Loop {
                count,
                body,
                until,
                col: -1,
                end: -1,
            },
            &start,
            &start,
        ))
    }
    fn branch(&mut self) -> AzaResult<Node> {
        let start = self.advance();
        let if_true = Box::new(self.branch_side("真")?);
        if !self.at_control("偽") {
            return Err(self
                .error("「真」のあとには「偽」も書いてください（例: 真 「はい」 偽 「いいえ」）"));
        }
        self.advance();
        let if_false = Box::new(self.branch_side("偽")?);
        Ok(mark(
            Node::Branch {
                if_true,
                if_false,
                col: -1,
                end: -1,
            },
            &start,
            &start,
        ))
    }
    fn branch_side(&mut self, word: &str) -> AzaResult<Node> {
        if self.at_control("終") {
            self.advance();
            return Ok(Node::End {});
        }
        if self.at_value() {
            return self.value();
        }
        if self.at_fn() {
            return self.phrase();
        }
        Err(self.error(format!("「{word}」のうしろには値か演算が必要です")))
    }
    fn phrase(&mut self) -> AzaResult<Node> {
        if !self.at_fn() {
            return Err(self.error("ここには演算が必要です"));
        }
        let function = self.function()?;
        let (monad, dyad) = self.arity(&function);
        let right = if self.at_value() && dyad {
            Some(Box::new(self.value()?))
        } else {
            if !monad {
                return Err(self.error(format!("「{}」の右に値が必要です", fn_label(&function))));
            }
            None
        };
        Ok(Node::Phrase {
            function: Box::new(function),
            right,
        })
    }
    fn function(&mut self) -> AzaResult<Node> {
        if self.at_anon(self.pos) {
            return self.anon();
        }
        let t = self.advance();
        if t.kind == "NAME" {
            return Ok(mark(named(&t.value), &t, &t));
        }
        let n = self.peek(0).clone();
        if t.kind == "FN" && t.value.chars().count() > MAX_WORD {
            return Err(self.error_at(
                format!("続けて書いた字が長すぎます（{MAX_WORD}字までです。演算と演算の間にはスペースを入れます）"),
                &t,
            ));
        }
        let adjacent = n.line == t.line && n.col == t.end;
        if t.kind == "FN"
            && all_modifiers(&t.value)
            && adjacent
            && n.kind == "NAME"
            && self.fn_names.contains_key(&n.value)
        {
            self.advance();
            let mut function = named(&n.value);
            for m in t.value.chars().rev() {
                function = self.combine(m, function, &format!("{}【{}】", t.value, n.value), &t)?;
            }
            return Ok(mark(function, &t, &n));
        }
        if t.kind == "FN" && all_modifiers(&t.value) && adjacent && self.at_anon(self.pos) {
            let mut function = self.anon()?;
            let last = self.tokens[self.pos - 1].clone();
            for m in t.value.chars().rev() {
                let label = format!("{}{}", t.value, fn_label(&function));
                function = self.combine(m, function, &label, &t)?;
            }
            return Ok(mark(function, &t, &last));
        }
        Ok(mark(self.build_fn(&t.value, &t)?, &t, &t))
    }
    fn anon(&mut self) -> AzaResult<Node> {
        self.enter_nest()?;
        let start = self.advance();
        let (mut depth, mut i) = (1isize, self.pos);
        while depth != 0
            && !matches!(
                self.tokens[i].kind.as_str(),
                "NEWLINE" | "EOF" | "PERIOD" | "COMMA"
            )
        {
            depth += match self.tokens[i].kind.as_str() {
                "LP" => 1,
                "RP" => -1,
                _ => 0,
            };
            i += 1;
        }
        let dyadic = self.uses_sore(self.pos, if depth == 0 { i - 1 } else { i });
        let outer = self.in_definition;
        self.in_definition = true;
        self.anon_depth += 1;
        let body = if self.at_value() {
            self.expr()
        } else {
            self.with_subject("これ".into(), |p| p.steps())
                .map(|steps| Node::Train { steps })
        };
        self.in_definition = outer;
        self.anon_depth -= 1;
        let body = Box::new(body?);
        if self.peek(0).kind != "RP" {
            let message = if matches!(
                self.peek(0).kind.as_str(),
                "NEWLINE" | "EOF" | "PERIOD" | "COMMA"
            ) {
                "（ に対応する ）がありません".into()
            } else {
                self.unexpected(self.peek(0))
            };
            return Err(self.error(message));
        }
        let last = self.advance();
        self.nest -= 1;
        Ok(mark(
            Node::AnonFn {
                body,
                dyadic,
                col: -1,
                end: -1,
            },
            &start,
            &last,
        ))
    }
    fn build_fn(&self, text: &str, t: &Token) -> AzaResult<Node> {
        if text.chars().count() > MAX_WORD {
            return Err(self.error_at(
                format!("続けて書いた字が長すぎます（{MAX_WORD}字までです。演算と演算の間にはスペースを入れます）"),
                t,
            ));
        }
        if let Some(message) = language()["reserved"].get(text).and_then(Value::as_str) {
            return Err(self.error_at(format!("「{text}」は{message}"), t));
        }
        if let Some(message) = language()["keyword_guide"]
            .get(text)
            .and_then(Value::as_str)
        {
            return Err(self.error_at(format!("「{text}」は{message}"), t));
        }
        if control(text) {
            return Err(self.error_at(
                format!("「{text}」は値のうしろに書きます（例: 1 回 10 自足）"),
                t,
            ));
        }
        if language()["primitives"].get(text).is_some() {
            return Ok(prim(text));
        }
        let chars: Vec<char> = text.chars().collect();
        if chars.first() == Some(&'内') {
            if chars.len() < 3 {
                return Err(self.error_at(
                    "「内」のうしろには演算を2つ書きます（例: 内足掛 → 行列の掛け算）",
                    t,
                ));
            }
            let reduce = self.build_fn(&chars[1].to_string(), t)?;
            let combine = self.build_fn(&chars[2..].iter().collect::<String>(), t)?;
            if !self.arity(&reduce).1 || !self.arity(&combine).1 {
                return Err(self.error_at(format!("「内」のうしろには、値を2つとる演算を2つ書きます（「{text}」は組み合わせられません）"), t));
            }
            return Ok(Node::InnerFn {
                reduce: Box::new(reduce),
                combine: Box::new(combine),
                col: -1,
                end: -1,
            });
        }
        if chars.len() > 1 && modifier(chars[0]) {
            return self.combine(
                chars[0],
                self.build_fn(&chars[1..].iter().collect::<String>(), t)?,
                text,
                t,
            );
        }
        if all_modifiers(text) {
            return Err(self.error_at(format!("「{text}」のうしろに演算が必要です（例: 各個。自分で作った操作なら 各【平均】のようにすき間なく書きます）"), t));
        }
        Err(self.error_at(
            format!("「{text}」は知らない演算です（名前なら【{text}】のように【】で囲みます）"),
            t,
        ))
    }
    fn combine(&self, m: char, base: Node, text: &str, t: &Token) -> AzaResult<Node> {
        let (monad, dyad) = self.arity(&base);
        // 熟語の前の字がうしろにとれる演算（conformance/language.json の modifier_rules）
        match modifier_rule(m, "base") {
            Some("dyad") if !dyad => {
                return Err(self.error_at(format!("「{m}」のうしろには、値を2つとる演算（足・掛・結 など）が来ます（「{text}」は組み合わせられません）"), t));
            }
            Some("monad") if !monad => {
                return Err(self.error_at(format!("「{m}」のうしろには、値を1つとる演算（数 など）が来ます（「{text}」は組み合わせられません）"), t));
            }
            _ => {}
        }
        Ok(Node::DerivedFn {
            modifier: m.to_string(),
            base: Box::new(base),
            col: -1,
            end: -1,
        })
    }
    fn arity(&self, node: &Node) -> (bool, bool) {
        match node {
            Node::PrimFn { name, .. } => {
                let p = &language()["primitives"][name];
                (
                    p["monad"].as_bool().unwrap_or(false),
                    p["dyad"].as_bool().unwrap_or(false),
                )
            }
            Node::DerivedFn { modifier, base, .. } => {
                match modifier
                    .chars()
                    .next()
                    .and_then(|m| modifier_rule(m, "arity"))
                {
                    Some("dyad") => (false, true),
                    Some("same") => self.arity(base),
                    Some("both") => (true, true),
                    _ => (true, false),
                }
            }
            Node::InnerFn { .. } => (false, true),
            Node::AnonFn { dyadic, .. } => (!dyadic, *dyadic),
            Node::NameFn { name, .. } => self.fn_names.get(name).copied().unwrap_or((true, false)),
            _ => (false, false),
        }
    }
}

fn mentions_arguments(node: &Node) -> bool {
    fn visit(v: &Value) -> bool {
        match v {
            Value::Object(obj) => {
                matches!(
                    obj.get("type").and_then(Value::as_str),
                    Some("KoreRef" | "SoreRef" | "KokoRef")
                ) || obj.values().any(visit)
            }
            Value::Array(items) => items.iter().any(visit),
            _ => false,
        }
    }
    visit(&serde_json::to_value(node).expect("serializable syntax tree"))
}

#[cfg(test)]
mod tests {
    use super::{parse_with_elisions, Elision};

    fn elisions(source: &str) -> Vec<(usize, usize, usize, String)> {
        parse_with_elisions(source)
            .unwrap()
            .1
            .into_iter()
            .map(
                |Elision {
                     line,
                     col,
                     end,
                     text,
                 }| (line, col, end, text),
            )
            .collect()
    }

    #[test]
    fn elided_subjects_are_reported_where_they_are_read() {
        // 【点数】 総足 割 個 → 割 （【点数】 個）
        assert_eq!(
            elisions("【点数】は 1 2\n【点数】 総足 割 個"),
            [(2, 10, 11, "【点数】".to_string())]
        );
        // 主語が（）でも、書いたとおりに補う
        assert_eq!(
            elisions("（1 2 3） 足 移 1"),
            [(1, 10, 11, "（1 2 3）".to_string())]
        );
        // 自分で作った操作の名前にも重ねる
        assert_eq!(
            elisions("【二倍】とは 掛 2\n1 2 足 【二倍】"),
            [(2, 6, 10, "1 2".to_string())]
        );
        // 操作の中では これ を補う
        assert_eq!(
            elisions("【平均】とは 総足 割 個"),
            [(1, 12, 13, "これ".to_string())]
        );
        // 説の中で演算から始まる手順は これ から読む
        assert_eq!(
            elisions("【f】とは 説 足 1、仮【a】は これ、引 【a】。"),
            [
                (1, 8, 9, "これ".to_string()),
                (1, 21, 22, "これ".to_string())
            ]
        );
        // 値1つでも2つでも使える演算は省略しない
        assert!(elisions("10 連 個").is_empty());
    }
}
