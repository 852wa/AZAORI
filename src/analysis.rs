//! Conservative, side-effect-free type, shape, and effect facts for the playground.
use crate::ast::Node;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Type {
    Integer,
    Float,
    Character,
    Nested,
    Operation,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Effect {
    MayError,
    ReadEnv,
    WriteEnv,
    Random,
    Input,
    HostRead,
    HostWrite,
    Sleep,
    Emit,
    End,
    PollStop,
}

#[derive(Clone, Debug)]
struct Fact {
    types: BTreeSet<Type>,
    shape: Option<Vec<Option<usize>>>,
    integer: Option<i64>,
    effects: BTreeSet<Effect>,
}

#[derive(Serialize)]
pub struct StatementAnalysis {
    pub line: usize,
    pub statement: String,
    pub types: Vec<&'static str>,
    pub shape: String,
    pub effects: Vec<&'static str>,
}

impl Fact {
    fn unknown() -> Self {
        Self {
            types: [Type::Unknown].into_iter().collect(),
            shape: None,
            integer: None,
            effects: [Effect::MayError].into_iter().collect(),
        }
    }

    fn typed(ty: Type, shape: Vec<Option<usize>>) -> Self {
        Self {
            types: [ty].into_iter().collect(),
            shape: Some(shape),
            integer: None,
            effects: BTreeSet::new(),
        }
    }

    fn integer(value: Option<i64>, shape: Vec<Option<usize>>) -> Self {
        let mut fact = Self::typed(Type::Integer, shape);
        fact.integer = value;
        fact
    }

    fn merge(&self, other: &Self) -> Self {
        let mut types = self.types.clone();
        types.extend(other.types.iter().copied());
        let shape = if self.shape == other.shape {
            self.shape.clone()
        } else {
            None
        };
        let mut effects = self.effects.clone();
        effects.extend(other.effects.iter().copied());
        Self {
            types,
            shape,
            integer: (self.integer == other.integer)
                .then_some(self.integer)
                .flatten(),
            effects,
        }
    }

    fn add_effect(&mut self, effect: Effect) {
        self.effects.insert(effect);
    }
}

pub fn analyze_program(program: &Node) -> Vec<StatementAnalysis> {
    let Node::Program { body } = program else {
        return Vec::new();
    };
    let mut env = HashMap::new();
    let mut result = Vec::with_capacity(body.len());
    for node in body {
        let (statement, mut fact) = match node {
            Node::Assign { name, expr, .. } => {
                let fact = infer(expr, &env, None, None);
                env.insert(name.clone(), fact.clone());
                (format!("【{name}】への代入"), fact)
            }
            Node::Define { name, .. } => {
                let fact = Fact::typed(Type::Operation, Vec::new());
                env.insert(name.clone(), fact.clone());
                (format!("【{name}】の定義"), fact)
            }
            Node::Show { expr, .. } => ("結果".into(), infer(expr, &env, None, None)),
            Node::EndLine { .. } => {
                let mut fact = Fact::typed(Type::Unknown, Vec::new());
                fact.add_effect(Effect::End);
                ("プログラム終了".into(), fact)
            }
            _ => ("不明な文".into(), Fact::unknown()),
        };
        if matches!(node, Node::Assign { .. } | Node::Define { .. }) {
            fact.add_effect(Effect::WriteEnv);
        }
        result.push(StatementAnalysis {
            line: node.line(),
            statement,
            types: fact.types.iter().map(|ty| ty.label()).collect(),
            shape: if fact.types.iter().all(|ty| *ty == Type::Operation) {
                "―".into() // 操作には形がない
            } else {
                shape_label(fact.shape.as_deref())
            },
            effects: fact.effects.iter().map(|effect| effect.label()).collect(),
        });
    }
    result
}

fn infer(
    node: &Node,
    env: &HashMap<String, Fact>,
    these: Option<&Fact>,
    those: Option<&Fact>,
) -> Fact {
    match node {
        Node::Strand { values } => {
            let mut types = BTreeSet::new();
            let mut integer = None;
            for value in values {
                if let Ok(n) = value.parse::<crate::values::Int>() {
                    types.insert(Type::Integer);
                    if values.len() == 1 {
                        integer = n.small();
                    }
                } else if value.parse::<f64>().is_ok() {
                    types.insert(Type::Float);
                } else {
                    types.insert(Type::Unknown);
                }
            }
            if types.is_empty() {
                types.insert(Type::Unknown);
            }
            Fact {
                types,
                shape: Some(if values.len() == 1 {
                    Vec::new()
                } else {
                    vec![Some(values.len())]
                }),
                integer,
                effects: BTreeSet::new(),
            }
        }
        Node::StrLit { text, .. } => Fact::typed(Type::Character, vec![Some(text.chars().count())]),
        Node::ItemList { items } => {
            let mut fact = Fact::typed(Type::Nested, vec![Some(items.len())]);
            for item in items {
                fact.effects.extend(infer(item, env, these, those).effects);
            }
            fact
        }
        Node::Group { expr } => infer(expr, env, these, those),
        Node::Expr { subject, steps } => {
            let mut value = infer(subject, env, these, those);
            let subject = value.clone();
            for step in steps {
                value = infer_step(step, &value, &subject, env);
            }
            value
        }
        Node::NameRef { name, .. } => env.get(name).cloned().unwrap_or_else(|| {
            let mut fact = Fact::unknown();
            fact.add_effect(Effect::ReadEnv);
            fact
        }),
        Node::KoreRef {} => these.cloned().unwrap_or_else(Fact::unknown),
        Node::AreRef {} => Fact::unknown(),
        Node::SoreRef {} => those.cloned().unwrap_or_else(Fact::unknown),
        Node::Acc {} | Node::KokoRef {} => Fact::unknown(),
        Node::MathOp { op, left, right } => {
            let right = infer(right, env, these, those);
            let left = left
                .as_deref()
                .map(|node| infer(node, env, these, those))
                .unwrap_or_else(|| Fact::integer(Some(0), Vec::new()));
            elementwise(op_type(op), &left, &right, true)
        }
        Node::AnonFn { body, .. } => {
            let mut fact = infer(body, env, these, those);
            fact.types = [Type::Operation].into_iter().collect();
            fact.shape = Some(Vec::new());
            fact
        }
        Node::Train { steps } => {
            let mut value = these.cloned().unwrap_or_else(Fact::unknown);
            let subject = value.clone();
            for step in steps {
                value = infer_step(step, &value, &subject, env);
            }
            value
        }
        Node::Phrase { function, right } => {
            let right = right.as_deref().map(|node| infer(node, env, these, those));
            apply_function(
                function,
                these.unwrap_or(&Fact::unknown()),
                right.as_ref(),
                env,
            )
        }
        Node::Block { .. } => Fact::unknown(),
        _ => Fact::unknown(),
    }
}

fn infer_step(step: &Node, acc: &Fact, subject: &Fact, env: &HashMap<String, Fact>) -> Fact {
    match step {
        Node::Step { function, right } => {
            let right = right
                .as_deref()
                .map(|node| infer(node, env, Some(subject), None));
            apply_function(function, acc, right.as_ref(), env)
        }
        Node::MathStep { expr } => infer(expr, env, Some(acc), None),
        Node::Branch {
            if_true, if_false, ..
        } => {
            // 条件（それまでの値）の効果も残す（「待」や「乱」で作った条件など）
            let mut fact = infer(if_true, env, Some(subject), None).merge(&infer(
                if_false,
                env,
                Some(subject),
                None,
            ));
            for effect in acc.effects.iter().copied() {
                fact.add_effect(effect);
            }
            fact
        }
        Node::Loop { body, .. } => {
            let mut fact = acc.merge(&infer(body, env, Some(acc), None));
            fact.add_effect(Effect::MayError);
            fact.add_effect(Effect::PollStop);
            fact
        }
        Node::End {} => {
            let mut fact = acc.clone();
            fact.add_effect(Effect::End);
            fact
        }
        Node::Pause {} => {
            let mut fact = acc.clone();
            fact.add_effect(Effect::Emit);
            fact
        }
        _ => Fact::unknown(),
    }
}

fn apply_function(
    function: &Node,
    left: &Fact,
    right: Option<&Fact>,
    env: &HashMap<String, Fact>,
) -> Fact {
    match function {
        Node::PrimFn { name, .. } => primitive(name, left, right),
        Node::DerivedFn { modifier, base, .. } => match modifier.as_str() {
            "自" => apply_function(base, left, Some(left), env),
            "反" => apply_function(base, right.unwrap_or(&Fact::unknown()), Some(left), env),
            "各" => {
                let cell = Fact {
                    shape: Some(Vec::new()),
                    integer: None,
                    ..left.clone()
                };
                let mut mapped = apply_function(base, &cell, None, env);
                mapped.shape = left.shape.clone();
                mapped.add_effect(Effect::PollStop);
                mapped
            }
            "非" => {
                let mut fact = apply_function(base, left, right, env);
                fact.types = [Type::Integer].into_iter().collect();
                fact.integer = None;
                fact.add_effect(Effect::MayError);
                fact
            }
            "総" | "最" | "累" => {
                let mut fact = apply_function(base, left, None, env);
                if modifier == "累" {
                    fact.shape = left.shape.clone();
                } else if let Some(shape) = &left.shape {
                    fact.shape = Some(
                        shape
                            .get(..shape.len().saturating_sub(1))
                            .unwrap_or(&[])
                            .to_vec(),
                    );
                }
                if modifier == "総" {
                    fact.add_effect(Effect::MayError);
                }
                fact
            }
            _ => {
                let mut fact = Fact::unknown();
                fact.effects.extend(left.effects.iter().copied());
                fact.effects.extend(base_effects(base));
                if let Some(right) = right {
                    fact.effects.extend(right.effects.iter().copied());
                }
                fact
            }
        },
        Node::AnonFn { body, .. } => infer(body, env, Some(left), right),
        Node::NameFn { .. } => {
            let mut fact = Fact::unknown();
            fact.add_effect(Effect::ReadEnv);
            fact
        }
        _ => Fact::unknown(),
    }
}

fn primitive(name: &str, left: &Fact, right: Option<&Fact>) -> Fact {
    let mut fact = match name {
        "連" if right.is_none() => {
            let length = left
                .integer
                .filter(|n| *n >= 0)
                .and_then(|n| usize::try_from(n).ok());
            let mut fact = Fact::typed(Type::Integer, vec![length]);
            fact.add_effect(Effect::MayError);
            fact
        }
        "個" => Fact::integer(None, Vec::new()),
        "深" => Fact::integer(None, Vec::new()),
        "初" => {
            let shape = left.shape.as_ref().map(|shape| {
                vec![shape
                    .first()
                    .copied()
                    .flatten()
                    .or_else(|| shape.is_empty().then_some(1))]
            });
            Fact {
                types: [Type::Integer].into_iter().collect(),
                shape,
                integer: None,
                effects: BTreeSet::new(),
            }
        }
        // 照・区 の結果は、左（データ）と同じ形
        "照" | "区" => Fact {
            types: [Type::Integer].into_iter().collect(),
            shape: left.shape.clone(),
            integer: None,
            effects: BTreeSet::new(),
        },
        "順" | "降" | "並" => {
            // 並べ替える位置は、左（データ）の最初の向きの長さ。右は並べる順を決める文字列
            let input = left;
            let shape = input
                .shape
                .as_ref()
                .map(|shape| vec![shape.first().copied().flatten()]);
            Fact {
                types: [Type::Integer].into_iter().collect(),
                shape,
                integer: None,
                effects: input.effects.clone(),
            }
        }
        "転" => Fact {
            types: left.types.clone(),
            shape: None,
            integer: None,
            effects: left.effects.clone(),
        },
        "外" | "外読" => {
            let mut fact = Fact::unknown();
            fact.add_effect(Effect::HostRead);
            fact
        }
        "外書" => {
            let mut fact = left.clone();
            fact.add_effect(Effect::HostWrite);
            fact
        }
        "乱" | "抽" => {
            let mut fact = Fact::unknown();
            fact.add_effect(Effect::Random);
            fact
        }
        "待" | "認" | "どれ" => {
            let mut fact = Fact::unknown();
            fact.add_effect(Effect::Input);
            fact
        }
        "置" => {
            let mut fact = left.clone();
            fact.add_effect(Effect::Sleep);
            fact
        }
        "足" | "引" | "掛" | "割" | "余" | "冪" | "倍" | "約" => {
            let unknown = Fact::unknown();
            let right = right.unwrap_or(&unknown);
            elementwise(numeric_types(name, left, right), left, right, true)
        }
        "大" | "小" | "等" | "異" | "超" | "未満" | "以上" | "以下" | "且" | "又" => {
            elementwise(
                [Type::Integer].into_iter().collect(),
                left,
                right.unwrap_or(&Fact::unknown()),
                true,
            )
        }
        "根" | "絶" => elementwise(left.types.clone(), left, left, true),
        _ => {
            let mut fact = Fact::unknown();
            fact.effects.extend(left.effects.iter().copied());
            fact.add_effect(Effect::MayError);
            fact
        }
    };
    fact.effects.extend(left.effects.iter().copied());
    if let Some(right) = right {
        fact.effects.extend(right.effects.iter().copied());
    }
    if may_error(name) {
        fact.add_effect(Effect::MayError);
    }
    fact
}

fn elementwise(types: BTreeSet<Type>, left: &Fact, right: &Fact, may_error: bool) -> Fact {
    let shape = match (&left.shape, &right.shape) {
        (Some(a), Some(b)) if a == b => Some(a.clone()),
        (Some(a), Some(b)) if a.is_empty() => Some(b.clone()),
        (Some(a), Some(b)) if b.is_empty() => Some(a.clone()),
        (Some(a), Some(b)) if a.iter().copied().product::<Option<usize>>() == Some(1) => {
            Some(b.clone())
        }
        (Some(a), Some(b)) if b.iter().copied().product::<Option<usize>>() == Some(1) => {
            Some(a.clone())
        }
        _ => None,
    };
    let mut fact = Fact {
        types,
        shape,
        integer: None,
        effects: left.effects.clone(),
    };
    fact.effects.extend(right.effects.iter().copied());
    if may_error {
        fact.add_effect(Effect::MayError);
    }
    fact
}

/// 記号の計算の答えになりうる型（÷ は 割 と同じく、いつも小数）
fn op_type(op: &str) -> BTreeSet<Type> {
    if op == "÷" {
        [Type::Float].into_iter().collect()
    } else {
        [Type::Integer, Type::Float].into_iter().collect()
    }
}

fn numeric_types(name: &str, left: &Fact, right: &Fact) -> BTreeSet<Type> {
    if name == "割" {
        return [Type::Float].into_iter().collect();
    }
    let types: Vec<_> = left.types.union(&right.types).copied().collect();
    if types.iter().all(|ty| *ty == Type::Integer) {
        [Type::Integer].into_iter().collect()
    } else if types
        .iter()
        .all(|ty| matches!(ty, Type::Integer | Type::Float))
    {
        [Type::Float].into_iter().collect()
    } else {
        [Type::Integer, Type::Float, Type::Unknown]
            .into_iter()
            .collect()
    }
}

fn may_error(name: &str) -> bool {
    !matches!(name, "個" | "深" | "初" | "降" | "順" | "並")
}

fn base_effects(node: &Node) -> BTreeSet<Effect> {
    match node {
        Node::PrimFn { name, .. } => match name.as_str() {
            "乱" | "抽" => [Effect::Random].into_iter().collect(),
            "待" | "認" | "どれ" => [Effect::Input].into_iter().collect(),
            "外" | "外読" => [Effect::HostRead].into_iter().collect(),
            "外書" => [Effect::HostWrite].into_iter().collect(),
            "置" => [Effect::Sleep].into_iter().collect(),
            _ => [Effect::MayError].into_iter().collect(),
        },
        Node::NameFn { .. } | Node::NameRef { .. } => {
            [Effect::ReadEnv, Effect::MayError].into_iter().collect()
        }
        Node::DerivedFn { base, .. } => base_effects(base),
        Node::AnonFn { body, .. } => node_effects(body),
        _ => [Effect::MayError].into_iter().collect(),
    }
}

fn node_effects(node: &Node) -> BTreeSet<Effect> {
    fn add(out: &mut BTreeSet<Effect>, node: &Node) {
        match node {
            Node::PrimFn { name, .. } => out.extend(base_effects(&Node::PrimFn {
                name: name.clone(),
                col: -1,
                end: -1,
            })),
            Node::NameFn { .. } | Node::NameRef { .. } => {
                out.extend([Effect::ReadEnv, Effect::MayError]);
            }
            Node::Pause {} => {
                out.insert(Effect::Emit);
            }
            Node::End {} => {
                out.insert(Effect::End);
            }
            Node::Assign { expr, .. }
            | Node::LocalAssign { expr, .. }
            | Node::Result { expr, .. }
            | Node::Show { expr, .. }
            | Node::Group { expr }
            | Node::MathStep { expr } => add(out, expr),
            Node::Define { train, .. } => add(out, train),
            Node::ItemList { items }
            | Node::Train { steps: items }
            | Node::Block { lines: items }
            | Node::Program { body: items } => {
                for child in items {
                    add(out, child);
                }
            }
            Node::Expr { subject, steps } => {
                add(out, subject);
                for child in steps {
                    add(out, child);
                }
            }
            Node::MathOp { left, right, .. } => {
                if let Some(left) = left {
                    add(out, left);
                }
                add(out, right);
            }
            Node::DerivedFn { base, .. } | Node::AnonFn { body: base, .. } => add(out, base),
            Node::InnerFn {
                reduce, combine, ..
            } => {
                add(out, reduce);
                add(out, combine);
            }
            Node::Phrase { function, right } | Node::Step { function, right } => {
                add(out, function);
                if let Some(right) = right {
                    add(out, right);
                }
            }
            Node::Loop {
                count, body, until, ..
            } => {
                if let Some(count) = count {
                    add(out, count);
                }
                add(out, body);
                if let Some(until) = until {
                    add(out, until);
                }
                out.insert(Effect::PollStop);
            }
            Node::Branch {
                if_true, if_false, ..
            } => {
                add(out, if_true);
                add(out, if_false);
            }
            _ => {}
        }
    }
    let mut effects = BTreeSet::new();
    add(&mut effects, node);
    effects
}

/// 形を、ほかの画面と同じ言葉で見せる（数1つ、長さ4のリスト、2行3列の表）。長さが分からない所は「いくつか」。
fn shape_label(shape: Option<&[Option<usize>]>) -> String {
    let n = |dim: &Option<usize>| {
        dim.map(|n| n.to_string())
            .unwrap_or_else(|| "いくつか".into())
    };
    match shape {
        None => "不明".into(),
        Some([]) => "数1つ".into(),
        Some([length]) => format!("長さ{}のリスト", n(length)),
        Some([rows, columns]) => format!("{}行{}列の表", n(rows), n(columns)),
        Some(dims) => format!(
            "{}の{}次元の値",
            dims.iter().map(n).collect::<Vec<_>>().join("×"),
            dims.len()
        ),
    }
}

impl Type {
    fn label(self) -> &'static str {
        match self {
            Self::Integer => "整数",
            Self::Float => "小数",
            Self::Character => "文字",
            Self::Nested => "入れ子",
            Self::Operation => "操作",
            Self::Unknown => "不明",
        }
    }
}

impl Effect {
    fn label(self) -> &'static str {
        match self {
            Self::MayError => "エラーになることがある",
            Self::ReadEnv => "名前を読む",
            Self::WriteEnv => "名前を付ける",
            Self::Random => "乱数を使う",
            Self::Input => "人に聞く",
            Self::HostRead => "外の世界を読む",
            Self::HostWrite => "ファイルに書く",
            Self::Sleep => "時間を置く",
            Self::Emit => "途中で表示する",
            Self::End => "プログラムを終える",
            Self::PollStop => "止めるボタンを確かめる",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_range_and_arithmetic_without_running_effects() {
        let program = crate::parser::parse("5 連 足 1\n1 乱").unwrap();
        let analysis = analyze_program(&program);
        assert_eq!(analysis[0].types, ["整数"]);
        assert_eq!(analysis[0].shape, "長さ5のリスト");
        assert!(analysis[0].effects.contains(&"エラーになることがある"));
        assert!(analysis[1].effects.contains(&"乱数を使う"));
    }

    #[test]
    fn data_is_on_the_left_for_matching_and_intervals() {
        let source = "「BANANA」 照 「AN」\n11 1 31 21 区 （10 20 30）\n1 2 非等 2";
        let program = crate::parser::parse(source).unwrap();
        let analysis = analyze_program(&program);
        assert_eq!(analysis[0].shape, "長さ6のリスト");
        assert_eq!(analysis[1].shape, "長さ4のリスト");
        assert_eq!(analysis[2].types, ["整数"]);
    }

    #[test]
    fn branch_keeps_the_effects_of_its_condition() {
        let program = crate::parser::parse("「x」 待 真 1 偽 2\n1 乱 真 1 偽 2").unwrap();
        let analysis = analyze_program(&program);
        assert!(analysis[0].effects.contains(&"人に聞く"));
        assert!(analysis[1].effects.contains(&"乱数を使う"));
    }
}
