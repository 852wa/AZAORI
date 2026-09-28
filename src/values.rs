use crate::error::AzaError;
pub use crate::int::{Int, IntoInt};
use num_bigint::BigInt;
use num_traits::{FromPrimitive, ToPrimitive};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::{
    ops::Deref,
    sync::{Arc, OnceLock},
};

pub const MAX_SIZE: usize = 1_000_000;
pub const MAX_DIGITS: usize = 10_000;
/// 入れ子の深さの上限（包 を何重にもかけたときなど）。表示や片付けが再帰するので、深すぎる値は作らない
pub const MAX_NESTING: usize = 1_000;
/// 1つの値の中の要素の総数（入れ子の中も数える）の上限
pub const MAX_TOTAL: usize = 10 * MAX_SIZE;
pub const DISPLAY_DIGITS: usize = 4_000;

#[derive(Clone, Debug)]
pub enum Elem {
    Int(Int),
    Float(f64),
    Char(char),
    Array(Box<Arr>),
}

impl Elem {
    pub fn int(n: impl IntoInt) -> Self {
        Self::Int(n.into_int())
    }
    pub fn to_json(&self) -> Value {
        match self {
            Self::Int(n) => json!({"type":"int", "value":n.to_string()}),
            Self::Float(n) if n.is_finite() => json!({"type":"float", "value":n}),
            Self::Float(n) => {
                json!({"type":"float", "value":if n.is_nan() {"nan"} else if n.is_sign_negative() {"-inf"} else {"inf"}})
            }
            Self::Char(c) => json!({"type":"char", "value":c.to_string()}),
            Self::Array(a) => json!({"type":"array", "value":a.to_json()}),
        }
    }
    pub fn from_json(value: &Value) -> Result<Self, AzaError> {
        if let Some(kind) = value.get("type").and_then(Value::as_str) {
            let v = &value["value"];
            return match kind {
                "int" => v
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .map(Self::Int)
                    .ok_or_else(|| AzaError::new("整数のJSONが正しくありません")),
                "float" => {
                    let n = v
                        .as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()));
                    n.map(Self::Float)
                        .ok_or_else(|| AzaError::new("小数のJSONが正しくありません"))
                }
                "char" => parse_char(v),
                "array" => Ok(Self::Array(Box::new(Arr::from_json(v)?))),
                _ => Err(AzaError::new("知らないJSONの値です")),
            };
        }
        if value.get("shape").is_some() {
            return Ok(Self::Array(Box::new(Arr::from_json(value)?)));
        }
        if let Some(n) = value.as_i64() {
            return Ok(Self::int(n));
        }
        if let Some(n) = value.as_u64() {
            return Ok(Self::int(n));
        }
        if let Some(n) = value.as_f64() {
            return Ok(Self::Float(n));
        }
        parse_char(value)
    }
}

fn parse_char(v: &Value) -> Result<Elem, AzaError> {
    if let Some(s) = v.as_str() {
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Ok(Elem::Char(c));
        }
    }
    Err(AzaError::new("文字のJSONが正しくありません"))
}

impl PartialEq for Elem {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b,
            (Self::Int(a), Self::Float(b)) | (Self::Float(b), Self::Int(a)) => {
                b.is_finite()
                    && b.fract() == 0.0
                    && BigInt::from_f64(*b).map(Int::from_big).as_ref() == Some(a)
            }
            (Self::Char(a), Self::Char(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
enum Storage {
    Ints(Arc<Vec<i64>>),
    Floats(Arc<Vec<f64>>),
    Chars(Arc<Vec<char>>),
    Boxed(Arc<Vec<Elem>>),
}

/// Homogeneous arrays use unboxed storage. The compatibility view is only
/// materialised for older general-purpose code paths that still need `Elem`.
#[derive(Clone, Debug)]
pub struct Data {
    storage: Storage,
    expanded: Arc<OnceLock<Vec<Elem>>>,
}

impl Data {
    pub fn from_vec(data: Vec<Elem>) -> Self {
        let storage = if data
            .iter()
            .all(|x| matches!(x, Elem::Int(n) if n.small().is_some()))
        {
            Storage::Ints(Arc::new(
                data.iter()
                    .map(|x| match x {
                        Elem::Int(n) => n.small().unwrap(),
                        _ => unreachable!(),
                    })
                    .collect(),
            ))
        } else if data.iter().all(|x| matches!(x, Elem::Float(_))) {
            Storage::Floats(Arc::new(
                data.iter()
                    .map(|x| match x {
                        Elem::Float(n) => *n,
                        _ => unreachable!(),
                    })
                    .collect(),
            ))
        } else if data.iter().all(|x| matches!(x, Elem::Char(_))) {
            Storage::Chars(Arc::new(
                data.iter()
                    .map(|x| match x {
                        Elem::Char(c) => *c,
                        _ => unreachable!(),
                    })
                    .collect(),
            ))
        } else {
            Storage::Boxed(Arc::new(data))
        };
        Self {
            storage,
            expanded: Arc::new(OnceLock::new()),
        }
    }

    pub fn chars(chars: Vec<char>) -> Self {
        Self {
            storage: Storage::Chars(Arc::new(chars)),
            expanded: Arc::new(OnceLock::new()),
        }
    }

    pub fn ints(ints: Vec<i64>) -> Self {
        Self {
            storage: Storage::Ints(Arc::new(ints)),
            expanded: Arc::new(OnceLock::new()),
        }
    }

    pub fn floats(floats: Vec<f64>) -> Self {
        Self {
            storage: Storage::Floats(Arc::new(floats)),
            expanded: Arc::new(OnceLock::new()),
        }
    }

    pub fn len(&self) -> usize {
        match &self.storage {
            Storage::Ints(xs) => xs.len(),
            Storage::Floats(xs) => xs.len(),
            Storage::Chars(xs) => xs.len(),
            Storage::Boxed(xs) => xs.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<Elem> {
        match &self.storage {
            Storage::Ints(xs) => xs.get(index).copied().map(Elem::int),
            Storage::Floats(xs) => xs.get(index).copied().map(Elem::Float),
            Storage::Chars(xs) => xs.get(index).copied().map(Elem::Char),
            Storage::Boxed(xs) => xs.get(index).cloned(),
        }
    }

    pub fn as_ints(&self) -> Option<&[i64]> {
        match &self.storage {
            Storage::Ints(xs) => Some(xs),
            _ => None,
        }
    }

    pub fn as_floats(&self) -> Option<&[f64]> {
        match &self.storage {
            Storage::Floats(xs) => Some(xs),
            _ => None,
        }
    }

    pub fn as_chars(&self) -> Option<&[char]> {
        match &self.storage {
            Storage::Chars(xs) => Some(xs),
            _ => None,
        }
    }

    pub fn has_nested(&self) -> bool {
        match &self.storage {
            Storage::Boxed(xs) => xs.iter().any(|x| matches!(x, Elem::Array(_))),
            _ => false,
        }
    }

    pub fn into_scalar(self) -> Elem {
        match self.storage {
            Storage::Ints(xs) => Elem::int(xs[0]),
            Storage::Floats(xs) => Elem::Float(xs[0]),
            Storage::Chars(xs) => Elem::Char(xs[0]),
            Storage::Boxed(xs) => xs[0].clone(),
        }
    }

    fn expanded(&self) -> &[Elem] {
        if let Storage::Boxed(xs) = &self.storage {
            return xs;
        }
        self.expanded.get_or_init(|| match &self.storage {
            Storage::Ints(xs) => xs.iter().copied().map(Elem::int).collect(),
            Storage::Floats(xs) => xs.iter().copied().map(Elem::Float).collect(),
            Storage::Chars(xs) => xs.iter().copied().map(Elem::Char).collect(),
            Storage::Boxed(_) => unreachable!(),
        })
    }
}

impl From<Vec<Elem>> for Data {
    fn from(data: Vec<Elem>) -> Self {
        Self::from_vec(data)
    }
}

impl Deref for Data {
    type Target = [Elem];
    fn deref(&self) -> &Self::Target {
        self.expanded()
    }
}

impl PartialEq for Data {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        match (&self.storage, &other.storage) {
            (Storage::Ints(a), Storage::Ints(b)) => a == b,
            (Storage::Floats(a), Storage::Floats(b)) => a == b,
            (Storage::Chars(a), Storage::Chars(b)) => a == b,
            (Storage::Boxed(a), Storage::Boxed(b)) => a == b,
            _ => (0..self.len()).all(|i| self.get(i) == other.get(i)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Arr {
    pub shape: Vec<usize>,
    /// Homogeneous values use primitive storage; boxed values remain shared.
    pub data: Data,
    pub kind: String,
}

impl Serialize for Arr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Arr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_json(&Value::deserialize(deserializer)?)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

impl PartialEq for Arr {
    fn eq(&self, other: &Self) -> bool {
        self.shape == other.shape && self.data == other.data
    }
}

impl Arr {
    pub fn new(shape: Vec<usize>, data: Vec<Elem>) -> Self {
        Self {
            shape,
            data: Data::from_vec(data),
            kind: String::new(),
        }
    }
    pub fn scalar(x: Elem) -> Self {
        Self::new(vec![], vec![x])
    }
    pub fn vector(xs: Vec<Elem>) -> Self {
        Self::new(vec![xs.len()], xs)
    }
    pub fn text(s: &str) -> Self {
        Self {
            shape: vec![s.chars().count()],
            data: Data::chars(s.chars().collect()),
            kind: "text".into(),
        }
    }
    pub fn rank(&self) -> usize {
        self.shape.len()
    }
    pub fn is_scalar(&self) -> bool {
        self.shape.is_empty()
    }
    pub fn to_json(&self) -> Value {
        json!({"shape":self.shape, "data":self.data.iter().map(Elem::to_json).collect::<Vec<_>>(), "kind":self.kind})
    }
    pub fn from_json(v: &Value) -> Result<Self, AzaError> {
        let shape = v["shape"]
            .as_array()
            .ok_or_else(|| AzaError::new("配列の形がありません"))?
            .iter()
            .map(|x| {
                x.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| AzaError::new("配列の形が正しくありません"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let size = shape_size(&shape)?;
        let raw = v["data"]
            .as_array()
            .ok_or_else(|| AzaError::new("配列の中身がありません"))?;
        if size != raw.len() {
            return Err(AzaError::new("配列の形と中身の個数が合いません"));
        }
        let data = raw.iter().map(Elem::from_json).collect::<Result<_, _>>()?;
        Ok(Self {
            shape,
            data: Data::from_vec(data),
            kind: v["kind"].as_str().unwrap_or("").into(),
        })
    }
}

pub fn to_json(a: &Arr) -> Value {
    a.to_json()
}
pub fn from_json(v: &Value) -> Result<Arr, AzaError> {
    Arr::from_json(v)
}
pub fn item(x: &Elem) -> Arr {
    match x {
        Elem::Array(a) => (**a).clone(),
        _ => Arr::scalar(x.clone()),
    }
}
pub fn pack(a: Arr) -> Elem {
    if a.is_scalar() {
        a.data.into_scalar()
    } else {
        Elem::Array(Box::new(a))
    }
}
pub fn is_string(a: &Arr) -> bool {
    a.rank() == 1 && a.data.as_chars().is_some() && (!a.data.is_empty() || a.kind == "text")
}
pub fn is_nested(a: &Arr) -> bool {
    a.data.has_nested()
}
pub fn is_number(x: &Elem) -> bool {
    matches!(x, Elem::Int(_) | Elem::Float(_))
}
pub fn is_whole(x: &Elem) -> bool {
    match x {
        Elem::Int(_) => true,
        Elem::Float(n) => n.is_finite() && n.fract() == 0.0,
        _ => false,
    }
}
pub fn whole(x: &Elem, who: &str) -> Result<BigInt, AzaError> {
    match x {
        Elem::Int(n) => Ok(n.to_big()),
        Elem::Float(n) if n.is_finite() && n.fract() == 0.0 => {
            BigInt::from_f64(*n).ok_or_else(|| AzaError::new("整数にできません"))
        }
        _ => Err(AzaError::new(format!(
            "「{who}」には整数を渡してください（{}がありました）",
            show_elem(x)
        ))),
    }
}
pub fn as_int(a: &Arr, who: &str) -> Result<BigInt, AzaError> {
    if a.rank() > 1 || a.data.len() != 1 {
        return Err(AzaError::new(format!(
            "「{who}」には数を1つ渡してください（{}がありました）",
            shape_text(a)
        )));
    }
    whole(&a.data[0], who)
}
pub fn as_list(a: &Arr, who: &str) -> Result<Vec<Elem>, AzaError> {
    if a.rank() > 1 {
        return Err(AzaError::new(format!(
            "「{who}」は表には使えません（{}がありました）",
            shape_text(a)
        )));
    }
    Ok(a.data.to_vec())
}
pub fn text_of(a: &Arr, who: &str) -> Result<String, AzaError> {
    if is_string(a)
        || (a.rank() == 1 && a.data.is_empty())
        || (a.is_scalar() && matches!(a.data.get(0), Some(Elem::Char(_))))
    {
        if let Some(chars) = a.data.as_chars() {
            return Ok(chars.iter().collect());
        }
        return Ok(a
            .data
            .iter()
            .filter_map(|x| {
                if let Elem::Char(c) = x {
                    Some(*c)
                } else {
                    None
                }
            })
            .collect());
    }
    Err(AzaError::new(format!(
        "「{who}」には文字を渡してください（{}がありました）",
        shape_text(a)
    )))
}
/// 入れ子の深さと、入れ子の中まで数えた要素の総数を確かめる（再帰しないでたどる）
pub fn check_nesting(a: &Arr) -> Result<(), AzaError> {
    if !a.data.has_nested() {
        return Ok(());
    }
    let mut total = a.data.len();
    let mut stack: Vec<(&Arr, usize)> = vec![(a, 1)];
    while let Some((arr, depth)) = stack.pop() {
        if depth > MAX_NESTING {
            return Err(AzaError::new(format!(
                "入れ子が深すぎます（値の中の値は{MAX_NESTING}段までです）"
            )));
        }
        if let Storage::Boxed(xs) = &arr.data.storage {
            for x in xs.iter() {
                if let Elem::Array(inner) = x {
                    total += inner.data.len();
                    if total > MAX_TOTAL {
                        return Err(AzaError::new(format!(
                            "要素が多すぎます（入れ子の中まで数えて{}万個までにしてください）",
                            MAX_TOTAL / 10_000
                        )));
                    }
                    if inner.data.has_nested() {
                        stack.push((inner, depth + 1));
                    }
                }
            }
        }
    }
    Ok(())
}
/// 入れ子の中まで数えた要素の総数（MAX_TOTAL を超えたら、そこで数えるのをやめる）
pub fn element_total(a: &Arr) -> usize {
    let mut total = a.data.len();
    let mut stack = vec![a];
    while let Some(arr) = stack.pop() {
        if let Storage::Boxed(xs) = &arr.data.storage {
            for x in xs.iter() {
                if let Elem::Array(inner) = x {
                    total += inner.data.len();
                    if total > MAX_TOTAL {
                        return total;
                    }
                    if inner.data.has_nested() {
                        stack.push(inner);
                    }
                }
            }
        }
    }
    total
}
/// 各・表・累 などで結果を集めるとき、入れ子の中まで数えた要素の総数が上限を超えていないか
pub fn add_total(total: &mut usize, a: &Arr) -> Result<(), AzaError> {
    *total = total.saturating_add(element_total(a));
    if *total > MAX_TOTAL {
        return Err(AzaError::new(format!(
            "要素が多すぎます（入れ子の中まで数えて{}万個までにしてください）",
            MAX_TOTAL / 10_000
        )));
    }
    Ok(())
}
pub fn check_size(n: usize) -> Result<(), AzaError> {
    if n > MAX_SIZE {
        Err(size_error(&BigInt::from(n)))
    } else {
        Ok(())
    }
}
/// 要素の数が多すぎるときのエラー。とても大きな数は桁数で言う（約N桁の数の個数）。
pub fn size_error(n: &BigInt) -> AzaError {
    let count = if *n < BigInt::from(1_000_000_000_000u64) {
        format!("{n}個")
    } else {
        format!(
            "約{}桁の数の個数",
            (n.bits() as f64 * std::f64::consts::LOG10_2) as u64 + 1
        )
    };
    AzaError::new(format!(
        "要素が多すぎます（{count}）。{MAX_SIZE}個までにしてください"
    ))
}
/// 大きな整数の個数を確かめて usize にする。
pub fn check_size_big(n: &BigInt) -> Result<usize, AzaError> {
    match n.to_usize() {
        Some(small) if small <= MAX_SIZE => Ok(small),
        _ => Err(size_error(n)),
    }
}
pub fn shape_size(shape: &[usize]) -> Result<usize, AzaError> {
    if shape.contains(&0) {
        return Ok(0);
    }
    let n = shape
        .iter()
        .try_fold(1usize, |a, b| a.checked_mul(*b))
        .ok_or_else(|| AzaError::new("要素が多すぎます"))?;
    check_size(n)?;
    Ok(n)
}
pub fn check_integer(n: BigInt) -> Result<Elem, AzaError> {
    if n.bits() > 33_220 && n.to_str_radix(10).trim_start_matches('-').len() > MAX_DIGITS {
        Err(AzaError::new(format!(
            "計算で数が大きすぎます（{MAX_DIGITS}桁まで）"
        )))
    } else {
        Ok(Elem::Int(Int::from_big(n)))
    }
}
pub fn number_f64(x: &Elem, who: &str) -> Result<f64, AzaError> {
    match x {
        Elem::Float(n) => Ok(*n),
        Elem::Int(n) => n
            .to_f64()
            .filter(|x| x.is_finite())
            .ok_or_else(|| AzaError::new(format!("「{who}」の計算で数が大きすぎます"))),
        _ => Err(AzaError::new(format!(
            "「{who}」は数にしか使えません（{}がありました）",
            show_elem(x)
        ))),
    }
}
pub fn compare_numbers(a: &Elem, b: &Elem) -> Option<Ordering> {
    match (a, b) {
        (Elem::Int(x), Elem::Int(y)) => Some(x.cmp(y)),
        (Elem::Float(x), Elem::Float(y)) => x.partial_cmp(y),
        (Elem::Int(x), Elem::Float(y)) => {
            if y.is_nan() {
                None
            } else if *y == f64::INFINITY {
                Some(Ordering::Less)
            } else if *y == f64::NEG_INFINITY {
                Some(Ordering::Greater)
            } else if let Some(v) = x.small().filter(|v| v.unsigned_abs() <= 1 << 53) {
                // 近道: 2の53乗までの整数は、小数にしても正確なので、そのまま比べられる
                (v as f64).partial_cmp(y)
            } else {
                let t = BigInt::from_f64(*y)?;
                Some(x.to_big().cmp(&t).then_with(|| {
                    if y.fract() > 0.0 {
                        Ordering::Less
                    } else if y.fract() < 0.0 {
                        Ordering::Greater
                    } else {
                        Ordering::Equal
                    }
                }))
            }
        }
        (Elem::Float(_), Elem::Int(_)) => compare_numbers(b, a).map(Ordering::reverse),
        _ => None,
    }
}
pub fn shape_text(a: &Arr) -> String {
    match a.shape.as_slice() {
        [] => "数1つ".into(),
        [n] => format!("長さ{n}のリスト"),
        [r, c] => format!("{r}行{c}列の表"),
        _ => format!(
            "{}の値",
            a.shape
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("×")
        ),
    }
}
pub fn fmt_num(x: &Elem) -> String {
    match x {
        Elem::Int(Int::Big(n)) if n.bits() > (3.32 * DISPLAY_DIGITS as f64) as u64 => {
            format!(
                "（約{}桁の整数）",
                (n.bits() as f64 * std::f64::consts::LOG10_2) as usize + 1
            )
        }
        Elem::Int(n) => n.to_string(),
        Elem::Float(n) => format_float(*n),
        Elem::Char(c) => c.to_string(),
        Elem::Array(a) => format_value(a),
    }
}
fn format_float(n: f64) -> String {
    if n.is_nan() {
        return "nan".into();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-inf" } else { "inf" }.into();
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{n:.0}").replace("-0", "0");
    }
    let sci = format!("{n:.9e}");
    let (mantissa, exp) = sci.split_once('e').unwrap();
    let exponent: i32 = exp.parse().unwrap();
    if !(-4..10).contains(&exponent) {
        format!(
            "{}e{}{:02}",
            mantissa.trim_end_matches('0').trim_end_matches('.'),
            if exponent < 0 { "-" } else { "+" },
            exponent.abs()
        )
    } else {
        let places = (9 - exponent).max(0) as usize;
        let s = format!("{n:.places$}");
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            s
        }
    }
}

pub fn code_num(x: &Elem) -> String {
    match x {
        Elem::Int(n) => n.to_string(),
        Elem::Float(n) => n.to_string(),
        _ => fmt_num(x),
    }
}
/// エラーの文に入れる値。とても大きな数や長い値は短くして見せる
pub fn show_elem(x: &Elem) -> String {
    if let Elem::Int(n) = x {
        let digits = n.to_string().trim_start_matches('-').len();
        if digits > 30 {
            return format!("約{digits}桁の数");
        }
    }
    let text = match x {
        Elem::Char(c) => format!("「{c}」"),
        _ => fmt_item(x),
    };
    if text.chars().count() > 80 {
        format!("{}…", text.chars().take(79).collect::<String>())
    } else {
        text
    }
}
pub fn fmt_item(x: &Elem) -> String {
    match x {
        Elem::Array(a) if is_string(a) => format!("「{}」", text_of(a, "表示").unwrap()),
        Elem::Array(a) if a.rank() == 1 && a.data.is_empty() => "「」".into(),
        Elem::Array(a) => format!("（{}）", format_value(a).replace('\n', " / ")),
        _ => fmt_num(x),
    }
}
pub fn format_value(a: &Arr) -> String {
    if !a.is_scalar() && a.data.is_empty() {
        return if a.kind == "text" { "" } else { "（空）" }.into();
    }
    if a.data.iter().all(|x| matches!(x, Elem::Char(_))) {
        if a.rank() <= 1 {
            return a.data.iter().map(fmt_num).collect::<Vec<_>>().concat();
        }
        return a
            .data
            .chunks(*a.shape.last().unwrap())
            .map(|r| r.iter().map(fmt_num).collect::<Vec<_>>().concat())
            .collect::<Vec<_>>()
            .join("\n");
    }
    let cells: Vec<_> = a.data.iter().map(fmt_item).collect();
    if a.is_scalar() {
        return cells.first().cloned().unwrap_or_default();
    }
    if a.rank() == 1 {
        return cells.join(" ");
    }
    let width = *a.shape.last().unwrap();
    if width == 0 {
        return "（空）".into();
    }
    let widths: Vec<_> = (0..width)
        .map(|j| {
            cells
                .iter()
                .skip(j)
                .step_by(width)
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut lines = Vec::new();
    for (i, row) in cells.chunks(width).enumerate() {
        if a.rank() > 2 && i > 0 && i % a.shape[a.rank() - 2] == 0 {
            lines.push(String::new());
        }
        lines.push(
            row.iter()
                .enumerate()
                .map(|(j, c)| format!("{}{c}", " ".repeat(widths[j] - c.chars().count())))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_json_preserves_nested_big_integers_and_empty_text() {
        let n = BigInt::from(10u8).pow(100);
        let value = Arr::vector(vec![
            Elem::int(n),
            Elem::Array(Box::new(Arr::text(""))),
            Elem::Float(f64::INFINITY),
        ]);
        let decoded: Arr = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(value, decoded);
        assert_eq!(value.to_json(), decoded.to_json());
        assert!(Arr::from_json(&json!({"shape":[2],"data":[1]})).is_err());
    }

    #[test]
    fn integer_float_comparison_does_not_round_the_integer() {
        let exact = Elem::Int("9007199254740993".parse().unwrap());
        let rounded = Elem::Float(9007199254740992.0);
        assert_ne!(exact, rounded);
        assert_eq!(compare_numbers(&exact, &rounded), Some(Ordering::Greater));
        assert_eq!(
            compare_numbers(&Elem::int(-2), &Elem::Float(-2.5)),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn display_formats_numbers_and_unicode_tables() {
        for (n, expected) in [
            (0.00001, "1e-05"),
            (1.23456789123, "1.234567891"),
            (1e20, "1e+20"),
            (-0.0, "0"),
            (1.2e-4, "0.00012"),
        ] {
            assert_eq!(fmt_num(&Elem::Float(n)), expected);
        }
        assert_eq!(format_value(&Arr::text("")), "");
        assert_eq!(format_value(&Arr::vector(vec![])), "（空）");
        assert_eq!(
            format_value(&Arr::new(
                vec![2, 2],
                vec![Elem::int(1), Elem::int(20), Elem::int(300), Elem::int(4)]
            )),
            "  1 20\n300  4"
        );
    }
}
