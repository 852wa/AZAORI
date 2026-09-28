//! Execution and derived functions. Host effects are isolated from language state.
mod machine;
use std::collections::HashMap;

use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};

use crate::ast::Node;
use crate::error::AzaError;
use crate::primitives;
use crate::values::{
    as_int, check_size, format_value, is_number, is_string, item, pack, shape_text, text_of, Arr,
    Elem, MAX_SIZE,
};
use crate::values::{fmt_num, show_elem};

pub const MAX_LOOP: usize = 100_000;
pub const MAX_DEPTH: usize = 5_000;
pub const MAX_PAUSES: usize = 100;
pub const MAX_FILE_BYTES: usize = 10_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub kind: String,
    pub line: usize,
    pub value: Arr,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputRequest {
    pub kind: String,
    pub prompt: String,
    pub options: Vec<String>,
}

pub trait Host {
    fn supports_input(&self, _kind: &str) -> bool {
        true
    }
    fn input(&mut self, request: &InputRequest) -> Result<String, AzaError> {
        Err(AzaError::new(unavailable_input(&request.kind)))
    }
    fn sleep(&mut self, seconds: f64) -> Result<(), AzaError> {
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
        #[cfg(target_arch = "wasm32")]
        if seconds > 0.0 {
            return Err(AzaError::new(
                "この実行環境では「置」の待機処理が提供されていません",
            ));
        }
        Ok(())
    }
    fn outside(&mut self, name: &str) -> Result<Arr, AzaError> {
        Err(AzaError::new(format!(
            "外の世界の「{name}」は、この実行環境では使えません"
        )))
    }
    fn read_file(&mut self, _name: &str) -> Result<String, AzaError> {
        Err(AzaError::new("この実行環境では「外読」は使えません"))
    }
    fn write_file(&mut self, _name: &str, _text: &str) -> Result<(), AzaError> {
        Err(AzaError::new("この実行環境では「外書」は使えません"))
    }
    fn stopped(&self) -> bool {
        false
    }
    fn event(&mut self, _event: &Event) {}
}

#[derive(Default)]
pub struct NullHost;
impl Host for NullHost {
    fn supports_input(&self, _kind: &str) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
enum Binding {
    Value(Arr),
    Function {
        body: Node,
        dyadic: bool,
        line: usize,
    },
}

/// 中身の記録: 操作の中で、これ・それ・あれ・ここ・仮の名前が何だったか（プレイグラウンドで見せる）。
/// ふだんは記録しない（`Interpreter::trace` が None）。同じ場所は最初の数回だけ残す。
#[derive(Clone, Debug, Default, Serialize)]
pub struct Trace {
    pub rows: Vec<TraceRow>,
    /// 場所（行・操作）ごとの呼ばれた回数（記録を省いた分も数える）
    pub calls: Vec<TraceCalls>,
    /// 行ごとの名前の中身（書き下しで、マウスを乗せたときに見せる）
    pub values: Vec<TraceValue>,
    #[serde(skip)]
    counts: HashMap<(usize, String), usize>,
    #[serde(skip)]
    value_map: HashMap<(usize, String), (Vec<String>, usize)>,
}

/// ある行で、ある名前（【点数】・これ・それ・あれ・ここ）が何だったか
#[derive(Clone, Debug, Serialize)]
pub struct TraceValue {
    pub line: usize,
    pub key: String,
    /// 最初の数回の中身（同じ中身が続くときは1つにまとめる）
    pub values: Vec<String>,
    /// その行でその名前を使った回数
    pub uses: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct TraceRow {
    pub line: usize,
    /// どの操作の中か（【分散】、（足 1） など）
    pub place: String,
    /// その場所の何回目の呼び出しか
    pub call: usize,
    /// どの行から呼ばれたか
    pub from: usize,
    /// [(名前, 中身)] 例: [("これ", "80 65 92 71"), ("仮【平均】", "77")]
    pub names: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TraceCalls {
    pub line: usize,
    pub place: String,
    pub calls: usize,
}

pub const TRACE_CALLS_SHOWN: usize = 3; // 同じ場所で残す呼び出しの数
pub const TRACE_ROWS: usize = 400; // 残す行の上限
const TRACE_ITEMS: usize = 30; // 中身として見せる要素の数

/// 中身を1行の短い文字にする（大きな配列は先頭だけ）。
pub fn trace_text(a: &Arr) -> String {
    if a.data.len() <= TRACE_ITEMS {
        return format_value(a).replace('\n', " / ");
    }
    let head = Arr::new(vec![TRACE_ITEMS], a.data[..TRACE_ITEMS].to_vec());
    format!(
        "{} …（{}）",
        format_value(&head).replace('\n', " / "),
        shape_text(a)
    )
}

#[derive(Debug)]
enum Signal {
    Error(AzaError),
    End,
}
impl From<AzaError> for Signal {
    fn from(error: AzaError) -> Self {
        Self::Error(error)
    }
}
type EvalResult = Result<Arr, Signal>;

pub struct Interpreter {
    env: HashMap<String, Binding>,
    pub results: Vec<(usize, Arr)>,
    pub events: Vec<Event>,
    pub pauses: usize,
    pub ended: bool,
    pub line: usize,
    pub host: Box<dyn Host>,
    args: Vec<(Arr, Option<Arr>, Option<Arr>)>,
    subjects: Vec<Arr>,
    accs: Vec<Arr>,
    positions: Vec<Arr>,
    locals: Vec<HashMap<String, Arr>>,
    /// 中身の記録（プレイグラウンド用。None なら記録しない）
    pub trace: Option<Trace>,
    /// 値に名前を付けた順（「変数の状態」を、書いた順に見せるため）
    names_in_order: Vec<String>,
    trace_line: usize,
    trace_places: Vec<(String, usize, usize)>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new(Box::new(NullHost))
    }
}

impl Interpreter {
    pub fn new(host: Box<dyn Host>) -> Self {
        Self {
            env: HashMap::new(),
            results: vec![],
            events: vec![],
            pauses: 0,
            ended: false,
            line: 0,
            host,
            args: vec![],
            subjects: vec![],
            accs: vec![],
            positions: vec![],
            locals: vec![],
            trace: None,
            names_in_order: vec![],
            trace_line: 0,
            trace_places: vec![],
        }
    }

    /// 操作に入ったときの これ・それ・あれ・ここ を記録する。記録しないときは何もしない。
    fn trace_enter(
        &mut self,
        place: String,
        from: usize,
        a: &Arr,
        b: Option<&Arr>,
        are: Option<&Arr>,
    ) {
        let Some(trace) = &mut self.trace else { return };
        let count = trace
            .counts
            .entry((self.trace_line, place.clone()))
            .or_insert(0);
        *count += 1;
        let call = *count;
        self.trace_places.push((place.clone(), call, from));
        let line = self.trace_line;
        let entry = trace.value_map.entry((line, "これ".into())).or_default();
        entry.1 += 1;
        if entry.0.len() < TRACE_CALLS_SHOWN && entry.0.last() != Some(&trace_text(a)) {
            entry.0.push(trace_text(a));
        }
        if let Some(b) = b {
            let entry = trace.value_map.entry((line, "それ".into())).or_default();
            entry.1 += 1;
            if entry.0.len() < TRACE_CALLS_SHOWN && entry.0.last() != Some(&trace_text(b)) {
                entry.0.push(trace_text(b));
            }
        }
        if call > TRACE_CALLS_SHOWN || trace.rows.len() >= TRACE_ROWS {
            return;
        }
        let mut names = vec![("これ".to_string(), trace_text(a))];
        if let Some(b) = b {
            names.push(("それ".into(), trace_text(b)));
        }
        if let Some(are) = are {
            names.push(("あれ".into(), trace_text(are)));
        }
        if let Some(position) = self.positions.last() {
            names.push(("ここ".into(), trace_text(position)));
        }
        trace.rows.push(TraceRow {
            line: self.trace_line,
            place,
            call,
            from,
            names,
        });
    }

    /// 行ごとの名前の中身を記録する（書き下しのマウスオン用）。
    fn trace_value(&mut self, key: &str, value: &Arr) {
        let line = self.trace_line;
        let Some(trace) = &mut self.trace else { return };
        let entry = trace.value_map.entry((line, key.to_string())).or_default();
        entry.1 += 1;
        if entry.0.len() < TRACE_CALLS_SHOWN {
            let text = trace_text(value);
            if entry.0.last() != Some(&text) {
                entry.0.push(text);
            }
        }
    }

    /// 手順の結果を記録する（書き下しの途中のまとまり用）。
    fn trace_step(&mut self, step: &Node, value: &Arr) {
        if self.trace.is_some() {
            if let Some(key) = step.step_key() {
                self.trace_value(&key, value);
            }
        }
    }

    fn trace_leave(&mut self) {
        if self.trace.is_some() {
            self.trace_places.pop();
        }
    }

    /// 説 の中で 仮 の名前に値を入れたときに記録する。
    fn trace_local(&mut self, line: usize, name: &str, value: &Arr) {
        if self.trace.is_none() {
            return;
        }
        self.trace_value(&format!("【{name}】"), value);
        let Some(trace) = &mut self.trace else { return };
        let Some((place, call, from)) = self.trace_places.last().cloned() else {
            return;
        };
        if call > TRACE_CALLS_SHOWN || trace.rows.len() >= TRACE_ROWS {
            return;
        }
        trace.rows.push(TraceRow {
            line,
            place,
            call,
            from,
            names: vec![(format!("仮【{name}】"), trace_text(value))],
        });
    }

    /// 名前を付けた値の、今の中身（プレイグラウンドの「変数の状態」）: [(名前, 中身, 形)]
    pub fn variables(&self) -> Vec<(String, String, String)> {
        self.names_in_order
            .iter()
            .filter_map(|name| match self.env.get(name) {
                Some(Binding::Value(value)) => {
                    Some((format!("【{name}】"), trace_text(value), shape_text(value)))
                }
                _ => None, // あとで同じ名前の操作を定義したもの
            })
            .collect()
    }

    /// 記録を仕上げる（場所ごとの呼ばれた回数を添える）。
    pub fn finish_trace(&mut self) -> Option<Trace> {
        let mut trace = self.trace.take()?;
        let mut calls: Vec<TraceCalls> = trace
            .counts
            .iter()
            .map(|((line, place), calls)| TraceCalls {
                line: *line,
                place: place.clone(),
                calls: *calls,
            })
            .collect();
        calls.sort_by(|a, b| (a.line, &a.place).cmp(&(b.line, &b.place)));
        trace.calls = calls;
        let mut values: Vec<TraceValue> = trace
            .value_map
            .drain()
            .map(|((line, key), (values, uses))| TraceValue {
                line,
                key,
                values,
                uses,
            })
            .collect();
        values.sort_by(|a, b| (a.line, &a.key).cmp(&(b.line, &b.key)));
        trace.values = values;
        Some(trace)
    }

    pub fn run(&mut self, program: &Node) -> Result<Vec<(usize, Arr)>, AzaError> {
        let Node::Program { body } = program else {
            return Err(AzaError::new("プログラムの構文木が必要です"));
        };
        for statement in body {
            self.line = node_line(statement);
            self.trace_line = self.line;
            match self.exec(statement) {
                Ok(_) => (),
                Err(Signal::End) => {
                    self.ended = true;
                    self.emit("終", Arr::text("ここでプログラムを終えました"));
                    break;
                }
                Err(Signal::Error(mut error)) => {
                    if error.line.is_none() {
                        error.line = Some(self.line);
                    }
                    return Err(error);
                }
            }
        }
        Ok(self.results.clone())
    }

    fn exec(&mut self, node: &Node) -> Result<(), Signal> {
        self.check_stop()?;
        match node {
            Node::Assign { name, expr, .. } => {
                let value = self.eval(expr)?;
                if self.trace.is_some() {
                    self.trace_value(&format!("【{name}】"), &value);
                }
                if !self.names_in_order.contains(name) {
                    self.names_in_order.push(name.clone());
                }
                self.env.insert(name.clone(), Binding::Value(value));
            }
            Node::Define {
                name,
                train,
                dyadic,
                ..
            } => {
                self.env.insert(
                    name.clone(),
                    Binding::Function {
                        body: (**train).clone(),
                        dyadic: *dyadic,
                        line: self.line,
                    },
                );
            }
            Node::Show { expr, .. } => {
                let value = self.eval(expr)?;
                self.results.push((self.line, value.clone()));
                self.emit("結果", value);
            }
            Node::EndLine { .. } => return Err(Signal::End),
            _ => return Err(AzaError::new("実行できない文です").into()),
        }
        Ok(())
    }

    fn emit(&mut self, kind: &str, value: Arr) {
        let event = Event {
            kind: kind.into(),
            line: self.line,
            text: format_value(&value),
            value,
        };
        self.host.event(&event);
        self.events.push(event);
    }

    fn check_stop(&self) -> Result<(), Signal> {
        if self.host.stopped() {
            let mut error = AzaError::new("実行を止めました");
            error.kind = "stopped".into();
            Err(error.into())
        } else {
            Ok(())
        }
    }

    fn eval(&mut self, node: &Node) -> EvalResult {
        match node {
            Node::Expr { subject, steps } => {
                let subject = self.value(subject)?;
                self.steps(steps, subject)
            }
            _ => self.value(node),
        }
    }

    fn steps(&mut self, steps: &[Node], subject: Arr) -> EvalResult {
        machine::run_steps(self, steps, subject)
    }

    fn value(&mut self, node: &Node) -> EvalResult {
        let result = self.value_inner(node);
        if self.trace.is_some() {
            // 書き下しのマウスオン用に、名前・これ・それ などの中身を行ごとに記録する
            if let Ok(value) = &result {
                let key = match node {
                    Node::NameRef { name, .. } => Some(format!("【{name}】")),
                    Node::KoreRef {} => Some("これ".into()),
                    Node::SoreRef {} => Some("それ".into()),
                    Node::AreRef {} => Some("あれ".into()),
                    Node::KokoRef {} => Some("ここ".into()),
                    _ => None,
                };
                if let Some(key) = key {
                    self.trace_value(&key, value);
                }
            }
        }
        result
    }

    fn value_inner(&mut self, node: &Node) -> EvalResult {
        match node {
            Node::Strand { values } => {
                let values = values
                    .iter()
                    .map(|value| parse_number(value))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(if values.len() == 1 {
                    Arr::scalar(values[0].clone())
                } else {
                    Arr::vector(values)
                })
            }
            Node::StrLit { text, .. } => Ok(Arr::text(text)),
            Node::ItemList { items } => {
                let mut out = Vec::with_capacity(items.len());
                for node in items {
                    let value = self.value(node)?;
                    out.push(if matches!(node, Node::Strand { .. }) {
                        pack(value)
                    } else {
                        Elem::Array(Box::new(value))
                    });
                }
                Ok(Arr::vector(out))
            }
            Node::NameRef { name, .. } => {
                if let Some(value) = self.locals.last().and_then(|locals| locals.get(name)) {
                    return Ok(value.clone());
                }
                match self.env.get(name) {
                    Some(Binding::Value(value)) => Ok(value.clone()),
                    Some(Binding::Function { .. }) => Err(AzaError::new(format!(
                        "【{name}】は操作です。値のうしろに置いて使います"
                    ))
                    .into()),
                    None => Err(place_error(
                        AzaError::new(format!("【{name}】はまだありません")).into(),
                        node,
                    )),
                }
            }
            Node::Group { expr } => self.eval(expr),
            Node::Expr { .. } => self.eval(node),
            Node::KoreRef {} => self
                .args
                .last()
                .map(|args| args.0.clone())
                .ok_or_else(|| AzaError::new("「これ」は操作の中で使います").into()),
            Node::SoreRef {} => self
                .args
                .last()
                .and_then(|args| args.1.clone())
                .ok_or_else(|| AzaError::new("「それ」に渡された右の値がありません").into()),
            Node::AreRef {} => self
                .args
                .last()
                .and_then(|args| args.2.clone())
                .ok_or_else(|| AzaError::new("「あれ」はその場の操作の中で使います").into()),
            Node::KokoRef {} => self.positions.last().cloned().ok_or_else(|| {
                AzaError::new(
                    "「ここ」は 各・毎 でかけている操作の中で使います（今は何番目でもありません）",
                )
                .into()
            }),
            Node::Acc {} => self
                .accs
                .last()
                .cloned()
                .ok_or_else(|| AzaError::new("計算の左の値がありません").into()),
            Node::MathOp { op, left, right } => {
                let a = if op == "負" {
                    Arr::scalar(Elem::int(0))
                } else {
                    self.value(
                        left.as_deref()
                            .ok_or_else(|| AzaError::new("計算の左の値がありません"))?,
                    )?
                };
                let b = self.value(right)?;
                let name = crate::parser::math_name(op)
                    .ok_or_else(|| AzaError::new("知らない計算記号です"))?;
                Ok(primitives::apply(name, &a, Some(&b))?)
            }
            _ => Err(AzaError::new("値として使えない構文です").into()),
        }
    }

    fn primitive(&mut self, name: &str, a: &Arr, b: Option<&Arr>) -> EvalResult {
        match name {
            "置" => {
                let seconds = b.unwrap_or(a);
                if seconds.data.len() != 1 || !is_number(&seconds.data[0]) {
                    return Err(AzaError::new(format!(
                        "「置」には秒数を1つ渡してください（{}がありました）",
                        shape_text(seconds)
                    ))
                    .into());
                }
                let mut remaining = numeric_f64(&seconds.data[0])?;
                if !(0.0..=10.0).contains(&remaining) {
                    return Err(AzaError::new(format!(
                        "「置」で置けるのは0〜10秒です（{}秒がありました）",
                        fmt_num(&seconds.data[0])
                    ))
                    .into());
                }
                while remaining > 0.0 {
                    self.check_stop()?;
                    let step = remaining.min(0.1);
                    self.host.sleep(step)?;
                    remaining -= step;
                }
                self.check_stop()?;
                Ok(a.clone())
            }
            "待" | "認" => {
                let mode = if name == "認" { "yesno" } else { "text" };
                if !self.host.supports_input(mode) {
                    return Err(AzaError::new(unavailable_input(mode)).into());
                }
                if b.is_some() {
                    return Err(AzaError::new(format!("「{name}」は値を1つとります")).into());
                }
                let mut prompt = a.clone();
                if prompt.rank() == 1 && prompt.data.as_chars().is_some() {
                    prompt.kind = "text".into();
                }
                self.emit(name, prompt);
                let answer = self.host.input(&InputRequest {
                    kind: mode.into(),
                    prompt: format_value(a),
                    options: vec![],
                })?;
                self.check_stop()?;
                if name == "認" {
                    let approved = matches!(answer.as_str(), "はい" | "true" | "1");
                    self.emit("入力", Arr::text(if approved { "はい" } else { "いいえ" }));
                    Ok(Arr::scalar(Elem::int(if approved { 1 } else { 0 })))
                } else {
                    let value = Arr::text(&answer);
                    self.emit("入力", value.clone());
                    Ok(value)
                }
            }
            "どれ" => {
                if !self.host.supports_input("choice") {
                    return Err(AzaError::new(
                        "ここでは「どれ」は使えません（選んでもらえる場所で実行してください）",
                    )
                    .into());
                }
                if a.rank() > 1 {
                    return Err(AzaError::new(format!(
                        "「どれ」の選択肢はリストで書きます（{}がありました）",
                        shape_text(a)
                    ))
                    .into());
                }
                if a.data.is_empty() {
                    return Err(AzaError::new("「どれ」の選択肢がありません").into());
                }
                let prompt = b
                    .map(format_value)
                    .unwrap_or_else(|| "どれにしますか？".into());
                let options = a
                    .data
                    .iter()
                    .map(|value| format_value(&item(value)))
                    .collect();
                self.emit("どれ", Arr::text(&prompt));
                let answer = self.host.input(&InputRequest {
                    kind: "choice".into(),
                    prompt,
                    options,
                })?;
                self.check_stop()?;
                let index = answer
                    .parse::<usize>()
                    .map_err(|_| AzaError::new("「どれ」で選ばれた番号が選択肢にありません"))?;
                let choice = a
                    .data
                    .get(index)
                    .ok_or_else(|| AzaError::new("「どれ」で選ばれた番号が選択肢にありません"))?;
                let value = item(&choice);
                self.emit("入力", value.clone());
                Ok(value)
            }
            "外" => {
                let name = text_of(a, "外")?;
                // 知っている名前かは、どの実行環境でも同じ文で知らせる（中身だけを Host に任せる）
                const SERVICES: [&str; 4] = ["時刻", "日付", "曜日", "秒"];
                if !SERVICES.contains(&name.as_str()) {
                    let names: Vec<String> = SERVICES.iter().map(|s| format!("\"{s}\"")).collect();
                    return Err(AzaError::new(format!(
                        "外の世界の「{name}」は知りません（使えるもの: {}）",
                        names.join("、")
                    ))
                    .into());
                }
                Ok(self.host.outside(&name)?)
            }
            "外読" => {
                let name = text_of(a, "外読")?;
                validate_file_name(&name)?;
                let text = self.host.read_file(&name)?;
                if text.len() > MAX_FILE_BYTES {
                    return Err(AzaError::new("ファイルが大きすぎます（10MBまで）").into());
                }
                let lines = split_text_lines(&text)?;
                Ok(Arr::vector(
                    lines
                        .into_iter()
                        .map(|line| Elem::Array(Box::new(Arr::text(line))))
                        .collect(),
                ))
            }
            "外書" => {
                let name = text_of(required_right(b, "外書")?, "外書")?;
                validate_file_name(&name)?;
                let text = if is_string(a) || a.data.is_empty() {
                    text_of(a, "外書")?
                } else if a.rank() == 1 {
                    a.data
                        .iter()
                        .map(|value| format_value(&item(value)))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    format_value(a)
                };
                self.host.write_file(&name, &(text + "\n"))?;
                Ok(a.clone())
            }
            _ => Ok(primitives::apply(name, a, b)?),
        }
    }
}

fn parse_number(text: &str) -> Result<Elem, AzaError> {
    if text.contains('.') || text.contains('e') || text.contains('E') {
        let value = text
            .parse::<f64>()
            .map_err(|_| AzaError::new("数を読めませんでした"))?;
        if !value.is_finite() {
            return Err(AzaError::new("数が大きすぎます"));
        }
        Ok(Elem::Float(value))
    } else {
        text.parse::<crate::values::Int>()
            .map(Elem::Int)
            .map_err(|_| AzaError::new("数を読めませんでした"))
    }
}

fn numeric_f64(value: &Elem) -> Result<f64, AzaError> {
    match value {
        Elem::Int(value) => value
            .to_f64()
            .ok_or_else(|| AzaError::new("数が大きすぎます")),
        Elem::Float(value) => Ok(*value),
        _ => Err(AzaError::new("数が必要です")),
    }
}

fn required_right<'a>(right: Option<&'a Arr>, name: &str) -> Result<&'a Arr, AzaError> {
    right.ok_or_else(|| AzaError::new(format!("「{name}」には右の値が必要です")))
}

fn check_arity(dyadic: bool, right: Option<&Arr>, name: &str) -> Result<(), AzaError> {
    if dyadic != right.is_some() {
        Err(AzaError::new(format!(
            "「{name}」は値を{}つとります",
            if dyadic { 2 } else { 1 }
        )))
    } else {
        Ok(())
    }
}

/// 近道: base が 足 などの要素どうしの演算で、x・y が入れ子でなければ、配列に包まずに計算する。
/// 包んで計算しても同じ scalar_dyad を通るので、結果もエラーの文も変わらない。
pub(crate) fn quick_pair(base: &Node, x: &Elem, y: &Elem) -> Option<Result<Elem, AzaError>> {
    match base {
        Node::PrimFn { name, .. }
            if primitives::is_elementwise(name)
                && !matches!(x, Elem::Array(_))
                && !matches!(y, Elem::Array(_)) =>
        {
            Some(primitives::scalar_dyad(name, x, y))
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum IntKernelOp {
    Add(i64),
    Sub(i64),
    Mul(i64),
    Double,
    Square,
}

impl IntKernelOp {
    pub(crate) fn apply(self, value: i64) -> Option<i64> {
        match self {
            Self::Add(right) => value.checked_add(right),
            Self::Sub(right) => value.checked_sub(right),
            Self::Mul(right) => value.checked_mul(right),
            Self::Double => value.checked_add(value),
            Self::Square => value.checked_mul(value),
        }
    }
}

fn integer_kernel_op(name: &str, right: i64) -> Option<IntKernelOp> {
    match name {
        "足" => Some(IntKernelOp::Add(right)),
        "引" => Some(IntKernelOp::Sub(right)),
        "掛" => Some(IntKernelOp::Mul(right)),
        _ => None,
    }
}

fn literal_small_integer(node: &Node) -> Option<i64> {
    let Node::Strand { values } = node else {
        return None;
    };
    let [literal] = values.as_slice() else {
        return None;
    };
    match parse_number(literal).ok()? {
        Elem::Int(value) => value.small(),
        _ => None,
    }
}

fn simple_each_integer_op(base: &Node) -> Option<IntKernelOp> {
    let Node::AnonFn {
        body,
        dyadic: false,
        ..
    } = base
    else {
        return None;
    };
    let Node::Train { steps } = body.as_ref() else {
        return None;
    };
    let [Node::Step {
        function,
        right: Some(right),
    }] = steps.as_slice()
    else {
        return None;
    };
    let Node::PrimFn { name, .. } = function.as_ref() else {
        return None;
    };
    integer_kernel_op(name, literal_small_integer(right)?)
}

fn kernel_operation(step: &Node) -> Option<IntKernelOp> {
    let Node::Step { function, right } = step else {
        return None;
    };
    match (function.as_ref(), right.as_deref()) {
        (Node::PrimFn { name, .. }, Some(right)) => {
            integer_kernel_op(name, literal_small_integer(right)?)
        }
        (Node::DerivedFn { modifier, base, .. }, None) if modifier == "自" => {
            match base.as_ref() {
                Node::PrimFn { name, .. } if name == "足" => Some(IntKernelOp::Double),
                Node::PrimFn { name, .. } if name == "掛" => Some(IntKernelOp::Square),
                _ => None,
            }
        }
        (Node::DerivedFn { modifier, base, .. }, None) if modifier == "各" => {
            simple_each_integer_op(base)
        }
        _ => None,
    }
}

fn is_primitive_step(step: &Node, name: &str) -> bool {
    matches!(step, Node::Step { function, right: None } if matches!(function.as_ref(), Node::PrimFn { name: found, .. } if found == name))
}

fn is_total_sum_step(step: &Node) -> bool {
    matches!(step, Node::Step { function, right: None } if matches!(function.as_ref(), Node::DerivedFn { modifier, base, .. } if modifier == "総" && matches!(base.as_ref(), Node::PrimFn { name, .. } if name == "足")))
}

pub(crate) struct IntegerRangeKernel {
    pub(crate) count: i64,
    pub(crate) operations: Vec<IntKernelOp>,
}

pub(crate) fn compile_range_kernel(steps: &[Node], count: i64) -> Option<IntegerRangeKernel> {
    if count <= 0 || usize::try_from(count).map_or(true, |n| n > MAX_SIZE) {
        return None;
    }
    if steps.len() < 2 || !is_primitive_step(&steps[0], "連") || !is_total_sum_step(steps.last()?)
    {
        return None;
    }
    let operations = steps[1..steps.len() - 1]
        .iter()
        .map(kernel_operation)
        .collect::<Option<Vec<_>>>()?;
    Some(IntegerRangeKernel { count, operations })
}

pub(crate) fn compile_range_kernel_from_node(program: &Node) -> Option<IntegerRangeKernel> {
    let Node::Program { body } = program else {
        return None;
    };
    let [Node::Show { expr, .. }] = body.as_slice() else {
        return None;
    };
    let Node::Expr { subject, steps } = expr.as_ref() else {
        return None;
    };
    compile_range_kernel(steps, literal_small_integer(subject)?)
}

/// Compile a deliberately small, checked integer kernel. Any unsupported or overflowing case
/// falls through to the ordinary evaluator, preserving arbitrary-precision semantics.
fn fuse_range_sum(
    steps: &[Node],
    subject: &Arr,
    interpreter: &mut Interpreter,
) -> Result<Option<Arr>, Signal> {
    if !subject.is_scalar() {
        return Ok(None);
    }
    let Some(&count) = subject.data.as_ints().and_then(|xs| xs.first()) else {
        return Ok(None);
    };
    let Some(kernel) = compile_range_kernel(steps, count) else {
        return Ok(None);
    };
    let mut total = 0i64;
    for value in 1..=kernel.count {
        if value % 65_536 == 0 {
            interpreter.check_stop()?;
        }
        let mut term = value;
        for operation in &kernel.operations {
            let Some(next) = operation.apply(term) else {
                return Ok(None);
            };
            term = next;
        }
        let Some(next) = total.checked_add(term) else {
            return Ok(None);
        };
        total = next;
    }
    Ok(Some(Arr::scalar(Elem::int(total))))
}

/// 入力を受け取れない場所で 待・認・どれ を使ったときの案内。
pub fn unavailable_input(kind: &str) -> String {
    match kind {
        "yesno" | "認" => {
            "ここでは「認」は使えません（はい・いいえを受け取れる場所で実行してください）".into()
        }
        "choice" | "どれ" => {
            "ここでは「どれ」は使えません（選んでもらえる場所で実行してください）".into()
        }
        _ => "ここでは「待」は使えません（入力を受け取れる場所で実行してください）".into(),
    }
}

fn truth(value: &Arr, word: &str) -> Result<bool, AzaError> {
    if value.data.len() != 1 || value.rank() > 1 {
        return Err(AzaError::new(format!(
            "「{word}」の条件は、1つの真（1）か偽（0）にしてください（{}がありました）。リスト全体なら「最小」（すべて）や「最大」（どれか）でまとめます",
            shape_text(value)
        )));
    }
    check_truth(&value.data[0], word)
}

fn check_truth(value: &Elem, word: &str) -> Result<bool, AzaError> {
    match value {
        Elem::Int(value) if value.is_zero() => Ok(false),
        Elem::Int(value) if *value == crate::values::Int::Small(1) => Ok(true),
        Elem::Float(value) if *value == 0.0 => Ok(false),
        Elem::Float(value) if *value == 1.0 => Ok(true),
        _ => Err(AzaError::new(format!(
            "「{word}」の前には、比べた結果（真は1、偽は0）が必要です（{}がありました）",
            show_elem(value)
        ))),
    }
}

fn spreads(one: &Arr, other: &Arr) -> bool {
    one.data.len() == 1 && (other.data.len() != 1 || other.rank() >= one.rank())
}

fn position(shape: &[usize], mut index: usize) -> Arr {
    if shape.len() <= 1 {
        return Arr::scalar(Elem::int(index + 1));
    }
    let mut coordinates = vec![Elem::int(0); shape.len()];
    for (axis, size) in shape.iter().enumerate().rev() {
        coordinates[axis] = Elem::int(index % size + 1);
        index /= size;
    }
    Arr::vector(coordinates)
}

pub fn validate_file_name(name: &str) -> Result<(), AzaError> {
    let normalized = name.replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized.split('/').any(|part| part == "..")
    {
        return Err(AzaError::new(format!(
            "ファイルは files フォルダの中だけ使えます（\"{name}\" は使えません）"
        )));
    }
    Ok(())
}

/// ファイルの中身を行に分ける（行が多すぎたら、分け終わる前にエラーにする）
fn split_text_lines(text: &str) -> Result<Vec<&str>, AzaError> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if matches!(
            ch,
            '\n' | '\r'
                | '\u{000b}'
                | '\u{000c}'
                | '\u{001c}'
                | '\u{001d}'
                | '\u{001e}'
                | '\u{0085}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            lines.push(&text[start..index]);
            check_size(lines.len())?;
            start = index + ch.len_utf8();
            if ch == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
                let (index, _) = chars.next().unwrap();
                start = index + 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
        check_size(lines.len())?;
    }
    Ok(lines)
}

fn node_line(node: &Node) -> usize {
    match node {
        Node::Assign { line, .. }
        | Node::Define { line, .. }
        | Node::Show { line, .. }
        | Node::EndLine { line }
        | Node::LocalAssign { line, .. }
        | Node::Result { line, .. } => *line,
        _ => 0,
    }
}

fn place_error(signal: Signal, node: &Node) -> Signal {
    let (col, end) = match node {
        Node::Step { function, .. } | Node::Phrase { function, .. } => {
            return place_error(signal, function)
        }
        Node::NameRef { col, end, .. }
        | Node::PrimFn { col, end, .. }
        | Node::DerivedFn { col, end, .. }
        | Node::NameFn { col, end, .. }
        | Node::InnerFn { col, end, .. }
        | Node::AnonFn { col, end, .. }
        | Node::Loop { col, end, .. }
        | Node::Branch { col, end, .. } => (*col, *end),
        _ => return signal,
    };
    match signal {
        Signal::Error(mut error) => {
            if error.kind == "stopped" {
                return Signal::Error(error);
            }
            if error.col.is_none() && col >= 0 {
                error.col = Some(col as usize);
                error.end = Some(end);
            }
            Signal::Error(error)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn run(source: &str) -> Result<Vec<String>, AzaError> {
        let program = crate::parser::parse(source)?;
        let mut interpreter = Interpreter::default();
        interpreter.run(&program).map(|results| {
            results
                .iter()
                .map(|(_, value)| format_value(value))
                .collect()
        })
    }

    #[test]
    fn arithmetic_and_omitted_subject() {
        assert_eq!(
            run("【点数】は 80 65 92 71\n【点数】 総足 割 個").unwrap(),
            ["77"]
        );
        assert_eq!(run("1 2 総足 - 1").unwrap(), ["2"]);
    }

    #[test]
    fn derived_fold_scan_outer_and_neighbor() {
        assert_eq!(
            run("10 3 2 総引\n1 2 3 累足\n1 3 6 10 隣引\n1 2 3 4 隣足 3").unwrap(),
            ["5", "1 3 6", "2 3 4", "6 9"]
        );
        assert_eq!(run("1 2 表掛 3 4").unwrap(), ["3 4\n6 8"]);
        assert_eq!(run("1 2 3 総（掛 10 足 それ）").unwrap(), ["123"]);
    }

    #[test]
    fn axes_and_inner_product() {
        assert_eq!(run("6 連 列 2 3 前総足").unwrap(), ["5 7 9"]);
        assert_eq!(run("1 2 3 内足掛 （4 5 6）").unwrap(), ["32"]);
        assert_eq!(
            run("【表】は 1 2 3 4 列 2 2\n1 前結 【表】").unwrap(),
            ["1 1\n1 2\n3 4"]
        );
    }

    #[test]
    fn are_and_positions() {
        assert_eq!(
            run("80 65 92 71 各（引 （あれ 総足 割 （あれ 個）））").unwrap(),
            ["3 -12 15 -6"]
        );
        assert_eq!(run("10 20 30 各（掛 ここ）").unwrap(), ["10 40 90"]);
        assert_eq!(
            run("10 毎（結 それ 結 ここ） 7 8").unwrap(),
            ["（10 7 1） （10 8 2）"]
        );
        assert_eq!(
            run("10 20 各（これ 毎（足 それ 足 ここ） 7）").unwrap(),
            ["18 28"]
        );
    }

    #[test]
    fn user_functions_and_local_names() {
        let source = "【点数】は 80 65 92 71\n【分散】とは 説\n  仮【中心】は 総足 割 個\n  引 【中心】 自掛 総足 割 個。\n【点数】 【分散】";
        assert_eq!(run(source).unwrap(), ["103.5"]);
        assert_eq!(run("【階乗】とは 説\n  仮【n】は これ\n  【n】 以下 1 真 1 偽 （【n】 掛 （【n】 引 1 【階乗】））。\n5 【階乗】").unwrap(), ["120"]);
    }

    #[test]
    fn recursion_depth_is_bounded() {
        let prefix = "【数える】とは これ 以下 0 真 0 偽 （これ 引 1 【数える】）\n";
        assert_eq!(
            run(&(prefix.to_owned() + "4999 【数える】")).unwrap(),
            ["0"]
        );
        assert!(run(&(prefix.to_owned() + "5001 【数える】"))
            .unwrap_err()
            .message
            .contains("呼び出しが深すぎます"));
    }

    #[test]
    fn array_branch_and_program_end() {
        assert_eq!(
            run("80 65 92 71 以上 70 真 「合格」 偽 「不合格」").unwrap(),
            ["「合格」 「不合格」 「合格」 「合格」"]
        );
        assert_eq!(run("1\n終\n2").unwrap(), ["1"]);
        assert!(run("1 0 真 終 偽 3").is_err());
    }

    struct Answers {
        answers: VecDeque<String>,
    }
    impl Host for Answers {
        fn input(&mut self, _: &InputRequest) -> Result<String, AzaError> {
            self.answers
                .pop_front()
                .ok_or_else(|| AzaError::new("test answers exhausted"))
        }
    }

    #[test]
    fn interactive_events_choose_confirm_and_text() {
        let program = crate::parser::parse(
            "「名前？」 待\n「りんご」 「みかん」 どれ\n「続ける？」 認 真 1 偽 終\n999",
        )
        .unwrap();
        let mut interpreter = Interpreter::new(Box::new(Answers {
            answers: ["花子", "1", "いいえ"]
                .into_iter()
                .map(String::from)
                .collect(),
        }));
        let results = interpreter.run(&program).unwrap();
        assert_eq!(
            results
                .iter()
                .map(|(_, value)| format_value(value))
                .collect::<Vec<_>>(),
            ["花子", "みかん"]
        );
        assert!(interpreter.ended);
        assert_eq!(interpreter.events.last().unwrap().kind, "終");
        assert_eq!(
            interpreter
                .events
                .iter()
                .filter(|event| event.kind == "入力")
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>(),
            ["花子", "みかん", "いいえ"]
        );
    }

    #[test]
    fn errors_have_call_site_and_local_context() {
        let error = run("【g】とは 説\n  仮【x】は これ 割 0\n  【x】。\n5 【g】").unwrap_err();
        assert_eq!(error.line, Some(4));
        assert_eq!(error.col, Some(2));
        assert!(error.message.ends_with("（【g】の1行目）"));
        assert_eq!(run("「abc」 試数 0").unwrap(), ["0"]);
    }

    #[test]
    fn text_line_splitting() {
        assert_eq!(split_text_lines("a\r\nb\rc\n").unwrap(), ["a", "b", "c"]);
        assert_eq!(split_text_lines("\n").unwrap(), [""]);
        assert!(split_text_lines("").unwrap().is_empty());
        assert!(split_text_lines(&"\n".repeat(2_000_000)).is_err());
    }

    #[test]
    fn unavailable_input_fails_before_emitting_a_prompt() {
        for source in ["「？」 待", "「？」 認", "1 2 どれ"] {
            let mut interpreter = Interpreter::default();
            let program = crate::parser::parse(source).unwrap();
            assert!(interpreter.run(&program).is_err());
            assert!(interpreter.events.is_empty());
        }
    }

    #[test]
    fn stopping_is_not_caught_by_attempt_and_has_no_error_span() {
        struct StopHost(usize);
        impl Host for StopHost {
            fn sleep(&mut self, _: f64) -> Result<(), AzaError> {
                self.0 += 1;
                Ok(())
            }
            fn stopped(&self) -> bool {
                self.0 >= 3
            }
        }
        let mut interpreter = Interpreter::new(Box::new(StopHost(0)));
        let program = crate::parser::parse("【f】とは 置 5\n1 試【f】 0").unwrap();
        let error = interpreter.run(&program).unwrap_err();
        assert_eq!(error.kind, "stopped");
        assert_eq!(error.line, Some(2));
        assert_eq!(error.col, None);
        assert_eq!(error.end, None);
    }
}
