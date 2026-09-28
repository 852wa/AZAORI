use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Node {
    Strand {
        values: Vec<String>,
    },
    StrLit {
        text: String,
        quote: String,
    },
    ItemList {
        items: Vec<Node>,
    },
    NameRef {
        name: String,
        col: i64,
        end: i64,
    },
    Group {
        expr: Box<Node>,
    },
    KoreRef {},
    SoreRef {},
    AreRef {},
    KokoRef {},
    Acc {},
    MathOp {
        op: String,
        left: Option<Box<Node>>,
        right: Box<Node>,
    },
    PrimFn {
        name: String,
        col: i64,
        end: i64,
    },
    DerivedFn {
        modifier: String,
        base: Box<Node>,
        col: i64,
        end: i64,
    },
    NameFn {
        name: String,
        col: i64,
        end: i64,
    },
    InnerFn {
        reduce: Box<Node>,
        combine: Box<Node>,
        col: i64,
        end: i64,
    },
    AnonFn {
        body: Box<Node>,
        dyadic: bool,
        col: i64,
        end: i64,
    },
    Phrase {
        #[serde(rename = "fn")]
        function: Box<Node>,
        right: Option<Box<Node>>,
    },
    Step {
        #[serde(rename = "fn")]
        function: Box<Node>,
        right: Option<Box<Node>>,
    },
    MathStep {
        expr: Box<Node>,
    },
    Loop {
        count: Option<Box<Node>>,
        body: Box<Node>,
        until: Option<Box<Node>>,
        col: i64,
        end: i64,
    },
    Branch {
        if_true: Box<Node>,
        if_false: Box<Node>,
        col: i64,
        end: i64,
    },
    Pause {},
    End {},
    Expr {
        subject: Box<Node>,
        steps: Vec<Node>,
    },
    Train {
        steps: Vec<Node>,
    },
    LocalAssign {
        name: String,
        expr: Box<Node>,
        line: usize,
    },
    Result {
        expr: Box<Node>,
        line: usize,
    },
    Block {
        lines: Vec<Node>,
    },
    Assign {
        name: String,
        expr: Box<Node>,
        line: usize,
    },
    Define {
        name: String,
        train: Box<Node>,
        line: usize,
        dyadic: bool,
    },
    Show {
        expr: Box<Node>,
        line: usize,
    },
    EndLine {
        line: usize,
    },
    Program {
        body: Vec<Node>,
    },
}

impl Node {
    pub fn line(&self) -> usize {
        match self {
            Self::Assign { line, .. }
            | Self::Define { line, .. }
            | Self::Show { line, .. }
            | Self::EndLine { line }
            | Self::LocalAssign { line, .. }
            | Self::Result { line, .. } => *line,
            _ => 0,
        }
    }
    /// 手順（Step・回・真偽）の目印の鍵「#列」。書き下しと実行の記録で同じものを使う（行は別に持つ）。
    pub fn step_key(&self) -> Option<String> {
        let node = match self {
            Self::Step { function, .. } => function.as_ref(),
            Self::Loop { .. } | Self::Branch { .. } => self,
            _ => return None,
        };
        node.span().map(|(col, _)| format!("#{col}"))
    }

    pub fn span(&self) -> Option<(usize, usize)> {
        let pair = match self {
            Self::NameRef { col, end, .. }
            | Self::PrimFn { col, end, .. }
            | Self::NameFn { col, end, .. }
            | Self::DerivedFn { col, end, .. }
            | Self::InnerFn { col, end, .. }
            | Self::AnonFn { col, end, .. }
            | Self::Loop { col, end, .. }
            | Self::Branch { col, end, .. } => (*col, *end),
            _ => (-1, -1),
        };
        (pair.0 >= 0 && pair.1 >= 0).then_some((pair.0 as usize, pair.1 as usize))
    }
}

pub fn fn_label(node: &Node) -> String {
    match node {
        Node::PrimFn { name, .. } => name.clone(),
        Node::NameFn { name, .. } => format!("【{}】", name),
        Node::DerivedFn { modifier, base, .. } => format!("{}{}", modifier, fn_label(base)),
        Node::InnerFn {
            reduce, combine, ..
        } => format!("内{}{}", fn_label(reduce), fn_label(combine)),
        Node::AnonFn { body, .. } => crate::expand::anonymous_label(body),
        _ => "演算".into(),
    }
}
