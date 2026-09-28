use crate::error::AzaError;
use crate::values::*;
use num_bigint::{BigInt, RandBigInt};
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{FromPrimitive, One, Signed, ToPrimitive, Zero};
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{HashSet, VecDeque},
};
use unicode_normalization::UnicodeNormalization;

thread_local! { static RANDOM: RefCell<StdRng> = RefCell::new(StdRng::from_entropy()); }
#[derive(Default)]
struct RandomInputs {
    draws: VecDeque<BigInt>,
    deals: VecDeque<Vec<usize>>,
    shuffles: VecDeque<Vec<usize>>,
}
thread_local! { static RANDOM_INPUTS: RefCell<RandomInputs> = RefCell::new(RandomInputs::default()); }
pub fn set_random_seed(seed: u64) {
    RANDOM.with(|r| *r.borrow_mut() = StdRng::seed_from_u64(seed));
}
pub fn configure_random(
    seed: u64,
    draws: Vec<i64>,
    deals: Vec<Vec<usize>>,
    shuffles: Vec<Vec<usize>>,
) {
    configure_random_big(
        seed,
        draws.into_iter().map(BigInt::from).collect(),
        deals,
        shuffles,
    );
}

pub fn configure_random_big(
    seed: u64,
    draws: Vec<BigInt>,
    deals: Vec<Vec<usize>>,
    shuffles: Vec<Vec<usize>>,
) {
    set_random_seed(seed);
    RANDOM_INPUTS.with(|r| {
        *r.borrow_mut() = RandomInputs {
            draws: draws.into(),
            deals: deals.into(),
            shuffles: shuffles.into(),
        }
    });
}

fn err(s: impl Into<String>) -> AzaError {
    AzaError::new(s)
}
fn numeric(x: &Elem, who: &str) -> Result<(), AzaError> {
    if is_number(x) {
        Ok(())
    } else {
        Err(err(format!(
            "「{who}」は数にしか使えません（{}がありました）",
            show_elem(x)
        )))
    }
}
fn boolean(x: &Elem, who: &str) -> Result<bool, AzaError> {
    if *x == Elem::int(0) {
        Ok(false)
    } else if *x == Elem::int(1) {
        Ok(true)
    } else {
        Err(err(format!(
            "「{who}」は真（1）か偽（0）に使います（{}がありました）",
            show_elem(x)
        )))
    }
}
fn bool_elem(b: bool) -> Elem {
    Elem::int(i32::from(b))
}
fn limit_usize(n: &BigInt) -> Result<usize, AzaError> {
    check_size_big(n)
}
fn integer_float(n: f64, name: &str) -> Result<Elem, AzaError> {
    if n.is_infinite() {
        return Err(err(format!("「{name}」の計算で数が大きすぎます")));
    }
    BigInt::from_f64(n)
        .ok_or_else(|| err(format!("「{name}」はこの値では計算できません")))
        .and_then(check_integer)
}
fn big_ln(n: &BigInt) -> f64 {
    if let Some(x) = n.to_f64().filter(|x| x.is_finite()) {
        return x.ln();
    }
    let shift = n.bits().saturating_sub(53) as usize;
    (n >> shift).to_f64().unwrap().ln() + shift as f64 * std::f64::consts::LN_2
}
fn number_ln(x: &Elem, name: &str) -> Result<f64, AzaError> {
    numeric(x, name)?;
    if compare_numbers(x, &Elem::int(0)) != Some(Ordering::Greater) {
        if matches!(x,Elem::Float(n) if n.is_nan()) {
            return Ok(f64::NAN);
        }
        return Err(err(format!(
            "0以下の数（{}）の対数は求められません",
            show_elem(x)
        )));
    }
    Ok(match x {
        Elem::Int(n) => big_ln(&n.to_big()),
        Elem::Float(n) => n.ln(),
        _ => unreachable!(),
    })
}

/// 近道で計算できる演算（名前を1回だけ調べて、要素ごとの文字列の比較を省く）
#[derive(Clone, Copy)]
enum Quick {
    Add,
    Sub,
    Mul,
    Div,
    Max,
    Min,
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

fn quick_op(name: &str) -> Option<Quick> {
    Some(match name {
        "足" => Quick::Add,
        "引" => Quick::Sub,
        "掛" => Quick::Mul,
        "割" => Quick::Div,
        "大" => Quick::Max,
        "小" => Quick::Min,
        "等" => Quick::Eq,
        "異" => Quick::Ne,
        "超" => Quick::Gt,
        "未満" => Quick::Lt,
        "以上" => Quick::Ge,
        "以下" => Quick::Le,
        _ => return None,
    })
}

/// 64ビットの整数どうし・小数どうしなら、確かめを省いて計算する（結果は普通の道と同じ）。
/// できないとき（はみ出す、0で割る、種類が混ざる など）は None を返し、普通の道に任せる。
#[inline]
fn quick(op: Quick, a: &Elem, b: &Elem) -> Option<Elem> {
    const EXACT: u64 = 1 << 53; // ここまでの整数は、小数にしても正確
    match (a, b) {
        (Elem::Int(Int::Small(x)), Elem::Int(Int::Small(y))) => {
            let (x, y) = (*x, *y);
            Some(match op {
                Quick::Add => Elem::int(x.checked_add(y)?),
                Quick::Sub => Elem::int(x.checked_sub(y)?),
                Quick::Mul => Elem::int(x.checked_mul(y)?),
                Quick::Div if y != 0 && x.unsigned_abs() <= EXACT && y.unsigned_abs() <= EXACT => {
                    Elem::Float(x as f64 / y as f64)
                }
                Quick::Div => return None,
                Quick::Max => Elem::int(if y > x { y } else { x }),
                Quick::Min => Elem::int(if y < x { y } else { x }),
                Quick::Eq => bool_elem(x == y),
                Quick::Ne => bool_elem(x != y),
                Quick::Gt => bool_elem(x > y),
                Quick::Lt => bool_elem(x < y),
                Quick::Ge => bool_elem(x >= y),
                Quick::Le => bool_elem(x <= y),
            })
        }
        (Elem::Float(x), Elem::Float(y)) => {
            let (x, y) = (*x, *y);
            Some(match op {
                Quick::Add => Elem::Float(x + y),
                Quick::Sub => Elem::Float(x - y),
                Quick::Mul => Elem::Float(x * y),
                Quick::Div if y != 0.0 => Elem::Float(x / y),
                Quick::Div => return None,
                Quick::Max => Elem::Float(if y > x { y } else { x }),
                Quick::Min => Elem::Float(if y < x { y } else { x }),
                Quick::Eq => bool_elem(x == y),
                Quick::Ne => bool_elem(x != y),
                Quick::Gt => bool_elem(x > y),
                Quick::Lt => bool_elem(x < y),
                Quick::Ge => bool_elem(x >= y),
                Quick::Le => bool_elem(x <= y),
            })
        }
        _ => None,
    }
}

pub fn scalar_dyad(name: &str, a: &Elem, b: &Elem) -> Result<Elem, AzaError> {
    if let Some(result) = quick_op(name).and_then(|op| quick(op, a, b)) {
        return Ok(result);
    }
    match name {
        "等" => return Ok(bool_elem(a == b)),
        "異" => return Ok(bool_elem(a != b)),
        "且" | "又" => {
            let x = boolean(a, name)?;
            let y = boolean(b, name)?;
            return Ok(bool_elem(if name == "且" { x && y } else { x || y }));
        }
        "倍" | "約" => {
            let x = whole(a, name)?;
            let y = whole(b, name)?;
            let result = if name == "倍" { x.lcm(&y) } else { x.gcd(&y) };
            return check_integer(result);
        }
        "超" | "未満" | "以上" | "以下" => {
            let order = match (a, b) {
                (Elem::Char(x), Elem::Char(y)) => Some(x.cmp(y)),
                (Elem::Char(_), _) | (_, Elem::Char(_)) => {
                    return Err(err(format!(
                        "「{name}」で数と文字は比べられません（{} と {}）",
                        show_elem(a),
                        show_elem(b)
                    )))
                }
                _ => compare_numbers(a, b),
            };
            return Ok(bool_elem(match name {
                "超" => order == Some(Ordering::Greater),
                "未満" => order == Some(Ordering::Less),
                "以上" => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
                _ => matches!(order, Some(Ordering::Less | Ordering::Equal)),
            }));
        }
        _ => {}
    }
    if name == "係" {
        return combination(a, b);
    }
    if name == "丸" {
        let d = whole(b,name).ok().and_then(|n| n.to_i32()).filter(|d|(-15..=15).contains(d)).ok_or_else(||err(format!("「丸」の右には、小数点以下の桁数（-15〜15の整数）を書いてください（{}がありました）",show_elem(b))))?;
        return round_half_up(a, d);
    }
    if name == "対" {
        // 左が0以下なら、右（底）を調べる前に「求められません」と知らせる。
        let bad = |x: &Elem| {
            matches!(
                compare_numbers(x, &Elem::int(0)),
                Some(Ordering::Less | Ordering::Equal)
            )
        };
        if !is_number(a) {
            number_f64(a, name)?;
        }
        if !bad(a) && !is_number(b) {
            number_f64(b, name)?;
        }
        if bad(a) || bad(b) || *b == Elem::int(1) {
            return Err(err(format!(
                "{}を底とする{}の対数は求められません",
                show_elem(b),
                show_elem(a)
            )));
        }
        return Ok(Elem::Float(number_ln(a, name)? / number_ln(b, name)?));
    }
    numeric(a, name)?;
    numeric(b, name)?;
    if name == "大" || name == "小" {
        let order = compare_numbers(b, a);
        return Ok(
            if (name == "大" && order == Some(Ordering::Greater))
                || (name == "小" && order == Some(Ordering::Less))
            {
                b.clone()
            } else {
                a.clone()
            },
        );
    }
    if matches!(name, "割" | "余") && *b == Elem::int(0) {
        return Err(err(if name == "割" {
            "0で割ることはできません"
        } else {
            "0で割った余りは求められません"
        }));
    }
    if let (Elem::Int(xi), Elem::Int(yi)) = (a, b) {
        // 近道: 両方とも64ビットに収まり、答えもはみ出さなければ、そのまま計算する
        if let (Int::Small(x), Int::Small(y)) = (xi, yi) {
            let (x, y) = (*x, *y);
            let fast = match name {
                "足" => x.checked_add(y),
                "引" => x.checked_sub(y),
                "掛" => x.checked_mul(y),
                "余" if y != 0 && y != -1 => Some(x.mod_floor(&y)),
                "余" if y == -1 => Some(0),
                _ => None,
            };
            if let Some(r) = fast {
                return Ok(Elem::int(r));
            }
        }
        let (xb, yb) = (xi.to_big(), yi.to_big());
        let (x, y) = (&xb, &yb);
        match name {
            "足" => return check_integer(x + y),
            "引" => return check_integer(x - y),
            "掛" => return check_integer(x * y),
            "余" => return Ok(Elem::int(x.mod_floor(y))),
            "割" => {
                return BigRational::new(x.clone(), y.clone())
                    .to_f64()
                    .filter(|x| x.is_finite())
                    .map(Elem::Float)
                    .ok_or_else(|| err("「割」の計算で数が大きすぎます"))
            }
            "冪" if y >= &BigInt::zero() => {
                if y.is_zero() {
                    return Ok(Elem::int(1));
                }
                if x.is_zero() {
                    return Ok(Elem::int(0));
                }
                if x.abs() == BigInt::one() {
                    return Ok(Elem::int(if x.is_negative() && y.is_odd() {
                        -1
                    } else {
                        1
                    }));
                }
                if y.to_f64().is_none_or(|f| !f.is_finite()) {
                    // 指数が小数にもできないほど大きい。
                    return Err(err("「冪」の計算で数が大きすぎます"));
                }
                let exponent = y.to_u32().ok_or_else(|| {
                    err(format!(
                        "{}の{}乗は大きすぎます（{MAX_DIGITS}桁まで）",
                        show_elem(a),
                        show_elem(b)
                    ))
                })?;
                if exponent as f64 * big_ln(&x.abs()) / std::f64::consts::LN_10 > MAX_DIGITS as f64
                {
                    return Err(err(format!(
                        "{}の{}乗は大きすぎます（{MAX_DIGITS}桁まで）",
                        show_elem(a),
                        show_elem(b)
                    )));
                }
                return check_integer(x.pow(exponent));
            }
            _ => {}
        }
    }
    if name == "冪" && is_number(a) && is_number(b) {
        // 小数にできないほど大きな数の累乗は、対数で大きさを見積もる。
        // 0 に近すぎる答えは 0、大きすぎる答えはエラーにする
        if number_f64(a, name).is_err() || number_f64(b, name).is_err() {
            return huge_power(a, b);
        }
    }
    let x = number_f64(a, name)?;
    let y = number_f64(b, name)?;
    let result = match name {
        "足" => x + y,
        "引" => x - y,
        "掛" => x * y,
        "割" => x / y,
        "余" => {
            let r = x % y;
            if r == 0.0 {
                0.0f64.copysign(y)
            } else if r.is_sign_negative() != y.is_sign_negative() {
                r + y
            } else {
                r
            }
        }
        "冪" => {
            if (x == 0.0 && y < 0.0)
                || (x < 0.0 && x.is_finite() && y.is_finite() && y.fract() != 0.0)
            {
                return Err(err(format!(
                    "{}の{}乗は計算できません",
                    show_elem(a),
                    show_elem(b)
                )));
            }
            let r = x.powf(y);
            if r.is_infinite() && x.is_finite() && y.is_finite() {
                return Err(err(format!(
                    "{}の{}乗は大きすぎます",
                    show_elem(a),
                    show_elem(b)
                )));
            }
            r
        }
        _ => return Err(err(format!("「{name}」は値2つの計算ではありません"))),
    };
    Ok(Elem::Float(result))
}

/// 小数にできないほど大きな数が入った累乗（答えは小数）
fn huge_power(a: &Elem, b: &Elem) -> Result<Elem, AzaError> {
    let cannot = || {
        err(format!(
            "{}の{}乗は計算できません",
            show_elem(a),
            show_elem(b)
        ))
    };
    let too_big = || {
        err(format!(
            "{}の{}乗は大きすぎます",
            show_elem(a),
            show_elem(b)
        ))
    };
    let ln_abs = |x: &Elem| match x {
        Elem::Int(n) => big_ln(&n.to_big().abs()),
        Elem::Float(f) => f.abs().ln(),
        _ => f64::NAN,
    };
    let negative = |x: &Elem| match x {
        Elem::Int(n) => n.is_negative(),
        Elem::Float(f) => *f < 0.0,
        _ => false,
    };
    let base_zero =
        matches!(a, Elem::Int(n) if n.is_zero()) || matches!(a, Elem::Float(f) if *f == 0.0);
    if base_zero {
        return if negative(b) {
            Err(cannot())
        } else {
            Ok(Elem::Float(0.0))
        };
    }
    // 負の数の累乗は、指数が整数のときだけ（符号は指数の偶奇で決まる）
    let odd = match b {
        Elem::Int(n) => n.to_big().is_odd(),
        Elem::Float(f) if f.is_finite() && f.fract() == 0.0 => (f % 2.0).abs() == 1.0,
        Elem::Float(_) => false,
        _ => return Err(cannot()),
    };
    if negative(a) && matches!(b, Elem::Float(f) if f.fract() != 0.0) {
        return Err(cannot());
    }
    let exponent = match b {
        Elem::Int(n) => n.to_f64().unwrap_or(if n.is_negative() {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }),
        Elem::Float(f) => *f,
        _ => return Err(cannot()),
    };
    let ln_base = ln_abs(a);
    let ln_result = if ln_base == 0.0 {
        0.0
    } else {
        exponent * ln_base
    };
    if ln_result.is_nan() {
        return Err(cannot());
    }
    if ln_result > 709.0 {
        return Err(too_big());
    }
    let magnitude = ln_result.exp();
    Ok(Elem::Float(if negative(a) && odd {
        -magnitude
    } else {
        magnitude
    }))
}
/// 整数 n を 10^k の倍数に四捨五入する（0 から遠いほうへ）
fn round_big(n: &BigInt, k: u32) -> BigInt {
    let unit = BigInt::from(10).pow(k);
    let (q, r) = n.abs().div_rem(&unit);
    let q = if r * 2 >= unit { q + 1 } else { q };
    let rounded = q * unit;
    if n.is_negative() {
        -rounded
    } else {
        rounded
    }
}
/// 四捨五入。小数は、書いたときの10進の数（1.005 なら 1.005）で丸める（二進数の誤差で 1.00 にならないように）
fn round_half_up(x: &Elem, d: i32) -> Result<Elem, AzaError> {
    if let Elem::Int(n) = x {
        if d <= 0 {
            return check_integer(round_big(&n.to_big(), d.unsigned_abs()));
        }
        // 小数点以下の桁を指定しても、整数はそのまま（大きな整数を小数にして精度を落とさない）
        if n.to_f64().is_none_or(|f| f.abs() > 9_007_199_254_740_992.0) {
            return Ok(x.clone());
        }
    }
    let n = number_f64(x, "丸")?;
    if !n.is_finite() {
        return integer_float(n, "丸");
    }
    // 最短の10進表記（1.005e0 など）を、整数の仮数と10の指数に分ける
    let text = format!("{:e}", n.abs());
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len() as i32);
    let digits: BigInt = mantissa.replace('.', "").parse().unwrap_or_default();
    // 値 = digits × 10^(exponent - fraction)。10^(-d) の位で丸める
    let shift = exponent - fraction + d;
    let rounded = if shift >= 0 {
        // もう丸める位より上の桁しかない。整数にするときは10進の数のまま（二進数の端数を出さない）
        return if d <= 0 {
            let whole = digits * BigInt::from(10).pow((exponent - fraction).max(0) as u32);
            check_integer(if n < 0.0 { -whole } else { whole })
        } else {
            Ok(Elem::Float(n))
        };
    } else if -shift > 400 {
        BigInt::zero()
    } else {
        round_big(&digits, (-shift) as u32) / BigInt::from(10).pow((-shift) as u32)
    };
    let value: f64 = format!("{rounded}e{}", -d).parse().unwrap_or(0.0);
    let value = value.copysign(n);
    if d <= 0 {
        integer_float(value, "丸")
    } else {
        Ok(Elem::Float(value))
    }
}
fn combination(a: &Elem, b: &Elem) -> Result<Elem, AzaError> {
    let message = || {
        err(format!(
            "「係」は0以上の整数に使います（{} と {}）",
            show_elem(a),
            show_elem(b)
        ))
    };
    let n = whole(a, "係").map_err(|_| message())?;
    let mut k = whole(b, "係").map_err(|_| message())?;
    if n.is_negative() || k.is_negative() {
        return Err(err(format!(
            "「係」は0以上の整数に使います（{} と {}）",
            show_elem(a),
            show_elem(b)
        )));
    }
    if k > n {
        return Ok(Elem::int(0));
    }
    // 答えの桁数を先に見積もり、上限を超えるなら計算しない
    k = std::cmp::min(k.clone(), &n - &k);
    // k ≤ n − k なので C(n,k) ≥ 2^k。k がこれより大きければ、答えは必ず上限の桁を超える
    let too_big = || {
        err(format!(
            "組み合わせの数は大きすぎます（{MAX_DIGITS}桁まで）"
        ))
    };
    let steps = k
        .to_usize()
        .filter(|k| *k as f64 * std::f64::consts::LOG10_2 <= MAX_DIGITS as f64 + 1.0)
        .ok_or_else(too_big)?;
    // 桁数 ≒ Σ log10((n − i) / (i + 1))。1つずつ足すので、n がとても大きくても桁落ちしない
    let log_n = big_ln(&n);
    let n_small = n.to_f64().filter(|x| x.is_finite() && *x < 1e15);
    let mut digits = 0.0;
    for i in 0..steps {
        let top = match n_small {
            Some(nf) => (nf - i as f64).ln(),
            None => log_n,
        };
        digits += (top - ((i + 1) as f64).ln()) / std::f64::consts::LN_10;
    }
    if digits > MAX_DIGITS as f64 + 1.0 {
        return Err(too_big());
    }
    let mut value = BigInt::one();
    for i in 1..=steps {
        value = value * (&n - BigInt::from(i - 1)) / BigInt::from(i);
        if value.bits() > 33_223 {
            return Err(err(format!(
                "組み合わせの数は大きすぎます（{MAX_DIGITS}桁まで）"
            )));
        }
    }
    check_integer(value)
}
fn scalar_monad(name: &str, x: &Elem) -> Result<Elem, AzaError> {
    if name == "否" {
        return Ok(bool_elem(!boolean(x, name)?));
    }
    if name == "階" && (!is_whole(x) || compare_numbers(x, &Elem::int(0)) == Some(Ordering::Less))
    {
        return Err(err(format!(
            "「階」は0以上の整数に使います（{}がありました）",
            show_elem(x)
        )));
    }
    if name == "乱" && (!is_whole(x) || compare_numbers(x, &Elem::int(1)) == Some(Ordering::Less))
    {
        return Err(err(format!(
            "「乱」には1以上の整数を渡してください（{}がありました）",
            show_elem(x)
        )));
    }
    numeric(x, name)?;
    match name {
        "絶" => {
            return match x {
                Elem::Int(n) => Ok(Elem::Int(n.abs())),
                Elem::Float(n) => Ok(Elem::Float(n.abs())),
                _ => unreachable!(),
            }
        }
        "丸" => return round_half_up(x, 0),
        "上" | "下" => {
            if let Elem::Int(_) = x {
                return Ok(x.clone());
            }
            let n = number_f64(x, name)?;
            return integer_float(if name == "上" { n.ceil() } else { n.floor() }, name);
        }
        "階" => {
            let n = whole(x, name)?;
            if n.is_negative() {
                return Err(err(format!(
                    "「階」は0以上の整数に使います（{}がありました）",
                    show_elem(x)
                )));
            }
            let n = n
                .to_u32()
                .filter(|n| *n <= 3000)
                .ok_or_else(|| err(format!("{}の階乗は大きすぎます（3000まで）", show_elem(x))))?;
            let mut f = BigInt::one();
            for i in 2..=n {
                f *= i;
            }
            return check_integer(f);
        }
        "乱" => {
            let n = whole(x, name)?;
            if n < BigInt::one() {
                return Err(err(format!(
                    "「乱」には1以上の整数を渡してください（{}がありました）",
                    show_elem(x)
                )));
            }
            if let Some(value) = RANDOM_INPUTS.with(|r| r.borrow_mut().draws.pop_front()) {
                return Ok(Elem::int(value));
            }
            return Ok(Elem::int(RANDOM.with(|r| {
                r.borrow_mut().gen_bigint_range(&BigInt::one(), &(&n + 1))
            })));
        }
        "対" => return Ok(Elem::Float(number_ln(x, name)?)),
        _ => {}
    }
    let n = number_f64(x, name).map_err(|e| {
        if matches!(name, "正弦" | "余弦" | "正接") {
            err(format!("{}の{name}は求められません", show_elem(x)))
        } else {
            e
        }
    })?;
    let r = match name {
        "根" => {
            if n < 0.0 {
                return Err(err(format!(
                    "マイナスの数（{}）の平方根は求められません",
                    show_elem(x)
                )));
            }
            n.sqrt()
        }
        "円" => n * std::f64::consts::PI,
        "正弦" | "余弦" | "正接" => {
            if n.is_infinite() {
                return Err(err(format!("{}の{name}は求められません", show_elem(x))));
            }
            let r = match name {
                "正弦" => n.sin(),
                "余弦" => n.cos(),
                _ => n.tan(),
            };
            if r.abs() < 1e-15 {
                return Ok(Elem::int(0));
            }
            r
        }
        _ => return Err(err(format!("「{name}」は値1つの計算ではありません"))),
    };
    Ok(Elem::Float(r))
}
fn pervasive1(name: &str, a: &Arr) -> Result<Arr, AzaError> {
    let data = (0..a.data.len())
        .map(|i| match a.data.get(i).expect("index within array") {
            Elem::Array(v) => Ok(pack(pervasive1(name, &v)?)),
            x => scalar_monad(name, &x),
        })
        .collect::<Result<_, AzaError>>()?;
    Ok(Arr::new(a.shape.clone(), data))
}
fn spreads(a: &Arr, b: &Arr) -> bool {
    a.data.len() == 1 && (b.data.len() != 1 || b.rank() >= a.rank())
}
/// 要素どうしで計算する、値2つの演算か（総足 などで、要素を配列に包まずに計算してよいもの）
pub fn is_elementwise(name: &str) -> bool {
    matches!(
        name,
        "足" | "引"
            | "掛"
            | "割"
            | "余"
            | "冪"
            | "大"
            | "小"
            | "丸"
            | "対"
            | "係"
            | "且"
            | "又"
            | "等"
            | "異"
            | "超"
            | "未満"
            | "以上"
            | "以下"
            | "倍"
            | "約"
    )
}

/// 型のそろった配列どうしの要素ごとの計算（quick と同じ結果になるものだけ）。
fn typed_kernel(op: Quick, a: &Data, b: &Data, n: usize) -> Option<Data> {
    const EXACT: u64 = 1 << 53; // ここまでの整数は、小数にしても正確
    let pick = |len: usize, i: usize| if len == 1 { 0 } else { i };
    if let (Some(xs), Some(ys)) = (a.as_ints(), b.as_ints()) {
        let pair = |i: usize| (xs[pick(xs.len(), i)], ys[pick(ys.len(), i)]);
        if let Quick::Div = op {
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let (x, y) = pair(i);
                if y == 0 || x.unsigned_abs() > EXACT || y.unsigned_abs() > EXACT {
                    return None;
                }
                out.push(x as f64 / y as f64);
            }
            return Some(Data::floats(out));
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let (x, y) = pair(i);
            out.push(match op {
                Quick::Add => x.checked_add(y)?,
                Quick::Sub => x.checked_sub(y)?,
                Quick::Mul => x.checked_mul(y)?,
                Quick::Div => unreachable!(),
                Quick::Max => {
                    if y > x {
                        y
                    } else {
                        x
                    }
                }
                Quick::Min => {
                    if y < x {
                        y
                    } else {
                        x
                    }
                }
                Quick::Eq => i64::from(x == y),
                Quick::Ne => i64::from(x != y),
                Quick::Gt => i64::from(x > y),
                Quick::Lt => i64::from(x < y),
                Quick::Ge => i64::from(x >= y),
                Quick::Le => i64::from(x <= y),
            });
        }
        return Some(Data::ints(out));
    }
    if let (Some(xs), Some(ys)) = (a.as_floats(), b.as_floats()) {
        let pair = |i: usize| (xs[pick(xs.len(), i)], ys[pick(ys.len(), i)]);
        let compare = matches!(
            op,
            Quick::Eq | Quick::Ne | Quick::Gt | Quick::Lt | Quick::Ge | Quick::Le
        );
        if compare {
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let (x, y) = pair(i);
                out.push(i64::from(match op {
                    Quick::Eq => x == y,
                    Quick::Ne => x != y,
                    Quick::Gt => x > y,
                    Quick::Lt => x < y,
                    Quick::Ge => x >= y,
                    _ => x <= y,
                }));
            }
            return Some(Data::ints(out));
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let (x, y) = pair(i);
            out.push(match op {
                Quick::Add => x + y,
                Quick::Sub => x - y,
                Quick::Mul => x * y,
                Quick::Div if y != 0.0 => x / y,
                Quick::Div => return None,
                Quick::Max => {
                    if y > x {
                        y
                    } else {
                        x
                    }
                }
                Quick::Min => {
                    if y < x {
                        y
                    } else {
                        x
                    }
                }
                _ => unreachable!(),
            });
        }
        return Some(Data::floats(out));
    }
    None
}

fn pervasive2(name: &str, a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let (shape, n) = if a.shape == b.shape {
        (a.shape.clone(), a.data.len())
    } else if spreads(a, b) {
        (b.shape.clone(), b.data.len())
    } else if spreads(b, a) {
        (a.shape.clone(), a.data.len())
    } else {
        return Err(err(format!(
            "「{name}」の左右の形が合いません（{} と {}）",
            shape_text(a),
            shape_text(b)
        )));
    };
    let op = quick_op(name);
    // 近道: 両方とも「全部64ビット整数」か「全部小数」なら、要素を包まずに専用のループで計算する。
    // はみ出し・0で割るなどに出会ったら None が返り、下の普通の道でやり直す（結果もエラーの文も同じ）。
    if let Some(op) = op {
        if let Some(data) = typed_kernel(op, &a.data, &b.data, n) {
            return Ok(Arr {
                shape,
                data,
                kind: String::new(),
            });
        }
    }
    let data = (0..n)
        .map(|i| {
            let x = a
                .data
                .get(if a.data.len() == 1 { 0 } else { i })
                .expect("broadcast index");
            let y = b
                .data
                .get(if b.data.len() == 1 { 0 } else { i })
                .expect("broadcast index");
            if let Some(result) = op.and_then(|op| quick(op, &x, &y)) {
                return Ok(result);
            }
            if matches!(x, Elem::Array(_)) || matches!(y, Elem::Array(_)) {
                Ok(pack(pervasive2(name, &item(&x), &item(&y))?))
            } else {
                scalar_dyad(name, &x, &y)
            }
        })
        .collect::<Result<_, AzaError>>()?;
    Ok(Arr::new(shape, data))
}

pub fn apply(name: &str, a: &Arr, b: Option<&Arr>) -> Result<Arr, AzaError> {
    if let Some(b) = b {
        return match name {
            "足" | "引" | "掛" | "割" | "余" | "冪" | "大" | "小" | "丸" | "対" | "係" | "且"
            | "又" | "等" | "異" | "超" | "未満" | "以上" | "以下" | "倍" | "約" => {
                pervasive2(name, a, b)
            }
            "同" => Ok(Arr::scalar(bool_elem(a == b))),
            "連" => span(a, b),
            "番" => pick(a, b),
            "拾" => pick_at(a, b),
            "替" => replace_at(a, b),
            "探" => find(a, b),
            "照" => find_pattern(b, a),
            "区" => interval_index(b, a),
            "順" => grade_by(b, a),
            "転" => transpose_axes(b, a),
            "含" => {
                let mut r = find(a, b)?;
                r.data = r
                    .data
                    .iter()
                    .map(|x| {
                        bool_elem(compare_numbers(x, &Elem::int(0)) == Some(Ordering::Greater))
                    })
                    .collect::<Vec<_>>()
                    .into();
                Ok(r)
            }
            "選" => compress(a, b),
            "取" | "捨" => take_drop(a, b, name),
            "結" => catenate(a, b),
            "列" => reshape(a, b),
            "分" => split(a, b),
            "組" => join(a, b),
            "共" | "抜" => set_filter(a, b, name),
            "移" => rotate(a, b),
            "位" => place(a, b),
            "別" => group(a, b),
            "換" => exchange(a, b),
            "方" => solve_system(a, b),
            "抽" => deal(a, b),
            "置" | "外書" | "どれ" => Err(err("この字は実行する環境の中でしか使えません")),
            _ => Err(err(format!("「{name}」は値を2つとる演算ではありません"))),
        };
    }
    match name {
        "根" | "丸" | "上" | "下" | "絶" | "階" | "対" | "円" | "正弦" | "余弦" | "正接" | "否"
        | "乱" => pervasive1(name, a),
        "連" => iota(a),
        "個" => Ok(Arr::scalar(Elem::int(if a.is_scalar() {
            1
        } else {
            a.shape[0]
        }))),
        "逆" => reverse(a),
        "並" | "順" | "降" => sort(a, name),
        "形" => Ok(Arr::vector(a.shape.iter().map(|n| Elem::int(*n)).collect())),
        "所" => where_positions(a),
        "唯" => unique(a),
        "初" => unique_mask(a),
        "深" => depth(a),
        "転" => permute(a, &(0..a.rank()).rev().collect::<Vec<_>>()),
        "包" => Ok(if a.is_scalar() {
            a.clone()
        } else {
            Arr::scalar(Elem::Array(Box::new(a.clone())))
        }),
        "平" => flatten(a),
        "位" => place(a, &Arr::scalar(Elem::int(10))),
        "編" => mix_split(a),
        "方" => inverse(a),
        "抽" => shuffle(a),
        "数" => to_number(a),
        "削" => trim(a),
        "検" => check(a),
        "置" | "待" | "認" | "外" | "外読" | "どれ" => {
            Err(err("この字は実行する環境の中でしか使えません"))
        }
        _ => Err(err(format!("「{name}」は値を1つとる演算ではありません"))),
    }
}

fn like(a: &Arr, data: Vec<Elem>) -> Arr {
    let mut r = Arr::vector(data);
    if is_string(a) {
        r.kind = "text".into();
    }
    r
}
/// a と同じ種類（文字列かどうか）の値を作る。文字でない要素が入ったら、もう文字列ではない
fn with_kind(a: &Arr, shape: Vec<usize>, data: Vec<Elem>) -> Arr {
    let text = a.kind == "text" && data.iter().all(|x| matches!(x, Elem::Char(_)));
    Arr {
        shape,
        data: data.into(),
        kind: if text || a.kind != "text" {
            a.kind.clone()
        } else {
            String::new()
        },
    }
}
fn iota(a: &Arr) -> Result<Arr, AzaError> {
    let n = as_int(a, "連")?;
    if n.is_negative() {
        return Err(err(format!(
            "「連」にはマイナスの数は使えません（{}がありました）",
            fmt_num(&Elem::int(n))
        )));
    }
    let n = limit_usize(&n)?;
    Ok(Arr {
        shape: vec![n],
        data: Data::ints((1..=n).map(|i| i as i64).collect()),
        kind: String::new(),
    })
}
fn span(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let x = as_int(a, "連")?;
    let y = as_int(b, "連")?;
    let n = limit_usize(&((&y - &x).abs() + 1))?;
    let step: i64 = if y >= x { 1 } else { -1 };
    if let (Some(start), Some(_)) = (x.to_i64(), y.to_i64()) {
        // 近道: 両端が64ビットに収まれば、途中もすべて収まる
        return Ok(Arr {
            shape: vec![n],
            data: Data::ints((0..n).map(|i| start + i as i64 * step).collect()),
            kind: String::new(),
        });
    }
    Ok(Arr::vector(
        (0..n)
            .map(|i| Elem::int(&x + BigInt::from(i) * step))
            .collect(),
    ))
}
fn reverse(a: &Arr) -> Result<Arr, AzaError> {
    if a.is_scalar() || a.shape.last() == Some(&0) {
        return Ok(a.clone());
    }
    let width = *a.shape.last().unwrap();
    let data = (0..a.data.len())
        .map(|i| {
            a.data
                .get((i / width) * width + width - 1 - i % width)
                .expect("index within array")
        })
        .collect();
    Ok(with_kind(a, a.shape.clone(), data))
}
fn sort(a: &Arr, name: &str) -> Result<Arr, AzaError> {
    let items = as_list(a, name)?;
    let strings = items
        .iter()
        .all(|x| matches!(x,Elem::Array(v) if is_string(v)));
    if !strings && items.iter().any(|x| matches!(x, Elem::Array(_))) {
        return Err(err(format!(
            "「{name}」は、文字列だけのリストか、数だけ・文字だけのリストに使えます"
        )));
    }
    let chars = items.iter().all(|x| matches!(x, Elem::Char(_)));
    if !chars && items.iter().any(|x| matches!(x, Elem::Char(_))) {
        return Err(err(format!(
            "「{name}」は、数と文字が混ざったリストには使えません"
        )));
    }
    let mut indices: Vec<usize> = (0..items.len()).collect();
    indices.sort_by(|i, j| {
        let order = match (&items[*i], &items[*j]) {
            (Elem::Array(x), Elem::Array(y)) => {
                text_of(x, name).unwrap().cmp(&text_of(y, name).unwrap())
            }
            (Elem::Char(x), Elem::Char(y)) => x.cmp(y),
            // NaN（数でない数）は、どの数よりもうしろ（全順序にしないと並べ替えが壊れる）
            (x, y) => compare_numbers(x, y).unwrap_or_else(|| {
                let nan = |v: &Elem| matches!(v, Elem::Float(f) if f.is_nan());
                nan(x).cmp(&nan(y))
            }),
        };
        if name == "降" {
            order.reverse()
        } else {
            order
        }
    });
    if matches!(name, "順" | "降") {
        Ok(Arr::vector(
            indices.into_iter().map(|i| Elem::int(i + 1)).collect(),
        ))
    } else {
        Ok(like(
            a,
            indices.into_iter().map(|i| items[i].clone()).collect(),
        ))
    }
}

/// 要素を「等しいか」で探すための鍵。Elem の == と同じ意味になるようにする
/// （整数の値の小数は整数と同じ鍵、NaN はどれとも等しくないので毎回ちがう鍵）
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Small(i64),
    Big(BigInt),
    Float(u64),
    Char(char),
    Array(Vec<usize>, Vec<Key>),
    Unique(usize),
}
struct Keys(usize);
impl Keys {
    fn of(&mut self, x: &Elem) -> Key {
        match x {
            Elem::Int(n) => n.small().map_or_else(|| Key::Big(n.to_big()), Key::Small),
            Elem::Float(f) if f.is_nan() => {
                self.0 += 1;
                Key::Unique(self.0)
            }
            Elem::Float(f) if f.is_finite() && f.fract() == 0.0 => match BigInt::from_f64(*f) {
                Some(n) => n.to_i64().map_or(Key::Big(n), Key::Small),
                None => Key::Float(f.to_bits()),
            },
            Elem::Float(f) => Key::Float(f.to_bits()),
            Elem::Char(c) => Key::Char(*c),
            Elem::Array(a) => self.array(a),
        }
    }
    fn array(&mut self, a: &Arr) -> Key {
        let keys = (0..a.data.len())
            .map(|i| self.of(&a.data.get(i).expect("index within the array")))
            .collect();
        Key::Array(a.shape.clone(), keys)
    }
}

fn unique_mask(a: &Arr) -> Result<Arr, AzaError> {
    let (rows, width) = if a.is_scalar() {
        (1, 1)
    } else {
        let rows = a.shape[0];
        (
            rows,
            match a.data.len().checked_div(rows) {
                Some(width) => width,
                None => shape_size(&a.shape[1..])?,
            },
        )
    };
    let mut mask = Vec::with_capacity(rows);
    let mut keys = Keys(0);
    let mut seen = HashSet::new();
    for row in 0..rows {
        let start = row * width;
        let key: Vec<Key> = (start..start + width)
            .map(|i| {
                a.data
                    .get(i)
                    .map_or(Key::Unique(usize::MAX), |x| keys.of(&x))
            })
            .collect();
        mask.push(bool_elem(seen.insert(key)));
    }
    Ok(Arr::vector(mask))
}

fn elem_depth(value: &Elem) -> usize {
    match value {
        Elem::Array(a) => array_depth(a),
        _ => 0,
    }
}

fn array_depth(a: &Arr) -> usize {
    if a.is_scalar() && a.data.get(0).is_some_and(|x| !matches!(x, Elem::Array(_))) {
        return 0;
    }
    if a.data.iter().all(|x| !matches!(x, Elem::Array(_))) {
        return 1;
    }
    1 + a.data.iter().map(elem_depth).max().unwrap_or(0)
}

fn depth(a: &Arr) -> Result<Arr, AzaError> {
    Ok(Arr::scalar(Elem::int(array_depth(a))))
}

/// エラーの文で「何があったか」を見せる（数1つなら値、それ以外は形）。
fn what(a: &Arr) -> String {
    if a.is_scalar() {
        show_elem(&a.data[0])
    } else {
        shape_text(a)
    }
}

fn grade_by(collation: &Arr, values: &Arr) -> Result<Arr, AzaError> {
    let order = collation
        .data
        .as_chars()
        .filter(|_| collation.rank() == 1)
        .ok_or_else(|| {
            err(format!(
                "「順」の右には、並べる順を文字列で書きます（{}がありました）",
                what(collation)
            ))
        })?;
    let chars = values
        .data
        .as_chars()
        .filter(|_| values.rank() > 0)
        .ok_or_else(|| {
            err(format!(
                "「順」の左には、並べる文字（文字列や文字の表）を書きます（{}がありました）",
                what(values)
            ))
        })?;
    let rows = values.shape[0];
    let width = match chars.len().checked_div(rows) {
        Some(width) => width,
        None => shape_size(&values.shape[1..])?,
    };
    let mut weights = std::collections::HashMap::new();
    for (index, c) in order.iter().enumerate() {
        weights.entry(*c).or_insert(index);
    }
    let mut indices: Vec<usize> = (0..rows).collect();
    indices.sort_by(|left, right| {
        let a = &chars[left * width..(left + 1) * width];
        let b = &chars[right * width..(right + 1) * width];
        a.iter()
            .map(|c| weights.get(c).copied().unwrap_or(order.len()))
            .cmp(
                b.iter()
                    .map(|c| weights.get(c).copied().unwrap_or(order.len())),
            )
    });
    Ok(Arr::vector(
        indices.into_iter().map(|i| Elem::int(i + 1)).collect(),
    ))
}

fn interval_index(bounds: &Arr, values: &Arr) -> Result<Arr, AzaError> {
    if bounds.rank() != 1 {
        return Err(err(format!(
            "「区」の右には、境界をリストで書きます（{}がありました）",
            what(bounds)
        )));
    }
    let boundaries: Vec<_> = (0..bounds.data.len())
        .map(|i| bounds.data.get(i).expect("boundary index"))
        .collect();
    for pair in boundaries.windows(2) {
        if simple_order(&pair[0], &pair[1])? == Ordering::Greater {
            return Err(err(format!(
                "「区」の境界は小さい順に並べてください（{}がありました）",
                format_value(bounds)
            )));
        }
    }
    let mut result = Vec::with_capacity(values.data.len());
    for index in 0..values.data.len() {
        let value = values.data.get(index).expect("value index");
        let mut low = 0;
        let mut high = boundaries.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if simple_order(&boundaries[middle], &value)? != Ordering::Greater {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        result.push(Elem::int(low));
    }
    Ok(Arr {
        shape: values.shape.clone(),
        data: result.into(),
        kind: String::new(),
    })
}

fn simple_order(a: &Elem, b: &Elem) -> Result<Ordering, AzaError> {
    match (a, b) {
        (Elem::Char(x), Elem::Char(y)) => Ok(x.cmp(y)),
        (Elem::Int(_) | Elem::Float(_), Elem::Int(_) | Elem::Float(_)) => {
            compare_numbers(a, b).ok_or_else(|| err("「区」は有限の数で比べてください"))
        }
        _ => Err(err(
            "「区」の境界と値は同じ種類（数または文字）にしてください",
        )),
    }
}
fn index(x: &Elem, len: usize) -> Result<usize, AzaError> {
    let n = whole(x, "番").ok().and_then(|n| n.to_usize());
    match n {
        Some(n) if n >= 1 && n <= len => Ok(n - 1),
        _ => Err(err(format!(
            "{}番目はありません（1〜{len}番目まであります）",
            show_elem(x)
        ))),
    }
}
fn pick(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let items = as_list(a, "番")?;
    let positions = as_list(b, "番")?;
    let data = positions
        .iter()
        .map(|p| Ok(items[index(p, items.len())?].clone()))
        .collect::<Result<Vec<_>, AzaError>>()?;
    Ok(if b.is_scalar() {
        item(&data[0])
    } else {
        like(a, data)
    })
}
fn find(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    if is_string(a) && is_string(b) {
        let text = text_of(a, "探")?;
        let target = text_of(b, "探")?;
        let n = text
            .find(&target)
            .map(|offset| text[..offset].chars().count() + 1)
            .unwrap_or(0);
        return Ok(Arr::scalar(Elem::int(n)));
    }
    let items = as_list(a, "探")?;
    let mut keys = Keys(0);
    let mut first = std::collections::HashMap::new();
    for (i, x) in items.iter().enumerate() {
        first.entry(keys.of(x)).or_insert(i + 1);
    }
    let mut pos = |target: &Elem| Elem::int(first.get(&keys.of(target)).copied().unwrap_or(0));
    if b.is_scalar() {
        Ok(Arr::scalar(pos(&b.data[0])))
    } else if is_string(b) {
        Ok(Arr::scalar(pos(&Elem::Array(Box::new(b.clone())))))
    } else {
        Ok(Arr::new(b.shape.clone(), b.data.iter().map(pos).collect()))
    }
}

fn coordinates(mut flat: usize, shape: &[usize]) -> Vec<usize> {
    let mut result = vec![0; shape.len()];
    for axis in (0..shape.len()).rev() {
        let width = shape[axis];
        if width == 0 {
            return result;
        }
        result[axis] = flat % width;
        flat /= width;
    }
    result
}

fn flat_index(indices: &[usize], shape: &[usize]) -> usize {
    indices
        .iter()
        .zip(shape)
        .fold(0usize, |flat, (index, width)| flat * width + index)
}

fn find_pattern(pattern: &Arr, values: &Arr) -> Result<Arr, AzaError> {
    let mut result = vec![Elem::int(0); values.data.len()];
    if pattern.rank() > values.rank() {
        return Ok(Arr {
            shape: values.shape.clone(),
            data: result.into(),
            kind: String::new(),
        });
    }
    let mut pattern_shape = vec![1; values.rank() - pattern.rank()];
    pattern_shape.extend(&pattern.shape);
    for (start, slot) in result.iter_mut().enumerate() {
        let origin = coordinates(start, &values.shape);
        if (0..values.rank()).any(|axis| origin[axis] + pattern_shape[axis] > values.shape[axis]) {
            continue;
        }
        let matches = (0..pattern.data.len()).all(|offset| {
            let relative = coordinates(offset, &pattern_shape);
            let target: Vec<_> = origin.iter().zip(relative).map(|(a, b)| a + b).collect();
            values.data.get(flat_index(&target, &values.shape)) == pattern.data.get(offset)
        });
        if matches {
            *slot = Elem::int(1);
        }
    }
    Ok(Arr {
        shape: values.shape.clone(),
        data: result.into(),
        kind: String::new(),
    })
}

fn counts(items: &[Elem], name: &str) -> Result<Vec<usize>, AzaError> {
    // まず全部を確かめてから、合計の大きさを確かめる。
    let mut values = Vec::with_capacity(items.len());
    for c in items {
        if !is_whole(c) || compare_numbers(c, &Elem::int(0)) == Some(Ordering::Less) {
            return Err(err(match name {
                "選" => format!(
                    "「選」の右には 1 か 0（または0以上の整数）を並べてください（{}がありました）",
                    show_elem(c)
                ),
                "所" => format!(
                    "「所」は 1 と 0（または0以上の整数）のリストに使います（{}がありました）",
                    show_elem(c)
                ),
                _ => format!(
                    "「{name}」には0以上の整数を並べてください（{}がありました）",
                    show_elem(c)
                ),
            }));
        }
        values.push(whole(c, name)?);
    }
    check_size_big(&values.iter().sum())?;
    Ok(values.iter().map(|n| n.to_usize().unwrap_or(0)).collect())
}
fn compress(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let items = as_list(a, "選")?;
    let raw = if b.rank() <= 1 && b.data.len() == 1 {
        vec![b.data[0].clone(); items.len()]
    } else if b.rank() == 1 && b.data.len() == items.len() {
        b.data.to_vec()
    } else {
        return Err(err(format!(
            "「選」の左右の長さが合いません（{} と {}）",
            shape_text(a),
            shape_text(b)
        )));
    };
    let counts = counts(&raw, "選")?;
    let mut out = Vec::new();
    for (x, n) in items.iter().zip(counts) {
        out.extend(std::iter::repeat_n(x.clone(), n));
    }
    Ok(like(a, out))
}
fn take_drop(a: &Arr, b: &Arr, name: &str) -> Result<Arr, AzaError> {
    let items = as_list(a, name)?;
    let n = as_int(b, name)?;
    let k = n.abs().to_usize().unwrap_or(usize::MAX).min(items.len());
    let data = match (name, n.is_negative()) {
        ("取", false) => items[..k].to_vec(),
        ("取", true) => items[items.len() - k..].to_vec(),
        (_, false) => items[k..].to_vec(),
        (_, true) => items[..items.len() - k].to_vec(),
    };
    Ok(like(a, data))
}
fn catenate(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    if a.rank() >= 2 || b.rank() >= 2 {
        return catenate_tables(a, b);
    }
    let mut left = as_list(a, "結")?;
    let mut right = as_list(b, "結")?;
    if is_string(a) && is_nested(b) {
        left = vec![Elem::Array(Box::new(a.clone()))];
    }
    if is_string(b) && is_nested(a) {
        right = vec![Elem::Array(Box::new(b.clone()))];
    }
    check_size(
        left.len()
            .checked_add(right.len())
            .ok_or_else(|| err("要素が多すぎます"))?,
    )?;
    left.extend(right);
    let mut result = Arr::vector(left);
    if is_string(a) && is_string(b) {
        result.kind = "text".into();
    }
    Ok(result)
}
fn catenate_tables(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let reference = if a.rank() >= 2 { a } else { b };
    let lead = &reference.shape[..reference.rank() - 1];
    let rows = shape_size(lead)?;
    let block = |x: &Arr| -> Result<Arr, AzaError> {
        if x.rank() == lead.len() + 1 {
            return Ok(x.clone());
        }
        let mut shape = lead.to_vec();
        shape.push(1);
        if x.is_scalar() {
            return Ok(Arr::new(shape, vec![x.data[0].clone(); rows]));
        }
        if x.shape == lead {
            return Ok(Arr::new(shape, x.data.to_vec()));
        }
        Err(err(format!(
            "「結」でつなげる形が合いません（{} と {}）",
            shape_text(a),
            shape_text(b)
        )))
    };
    let left = block(a)?;
    let right = block(b)?;
    if left.shape[..left.rank() - 1] != right.shape[..right.rank() - 1] {
        return Err(err(format!(
            "「結」でつなげる形が合いません（{} と {}）",
            shape_text(a),
            shape_text(b)
        )));
    }
    let wa = *left.shape.last().unwrap();
    let wb = *right.shape.last().unwrap();
    let width = wa.checked_add(wb).ok_or_else(|| err("要素が多すぎます"))?;
    let mut shape = lead.to_vec();
    shape.push(width);
    let size = shape_size(&shape)?;
    let mut data = Vec::with_capacity(size);
    for i in 0..rows {
        data.extend_from_slice(&left.data[i * wa..(i + 1) * wa]);
        data.extend_from_slice(&right.data[i * wb..(i + 1) * wb]);
    }
    Ok(Arr::new(shape, data))
}
fn reshape(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let raw = as_list(b, "列")?;
    let mut dims = Vec::with_capacity(raw.len());
    for x in raw {
        let n = whole(&x, "列")
            .ok()
            .filter(|n| !n.is_negative())
            .ok_or_else(|| {
                err("「列」の右には、0以上の整数で形を書いてください（例: 2 3 → 2行3列）")
            })?;
        dims.push(n);
    }
    // 形の積を大きな整数のまま確かめる（とても大きな形でも、個数をそのまま伝えるため）
    let total = check_size_big(&dims.iter().product())?;
    let shape: Vec<usize> = dims.iter().map(|n| n.to_usize().unwrap_or(0)).collect();
    if total > 0 && a.data.is_empty() {
        return Err(err("空のリストは並べ直せません"));
    }
    Ok(Arr::new(
        shape,
        (0..total)
            .map(|i| a.data.get(i % a.data.len()).expect("reshape index"))
            .collect(),
    ))
}
pub fn permute(a: &Arr, order: &[usize]) -> Result<Arr, AzaError> {
    if order.len() != a.rank()
        || order.iter().copied().collect::<HashSet<_>>().len() != a.rank()
        || order.iter().any(|i| *i >= a.rank())
    {
        return Err(err("軸の並びが正しくありません"));
    }
    let shape: Vec<_> = order.iter().map(|i| a.shape[*i]).collect();
    let size = shape_size(&shape)?;
    let mut strides = vec![1usize; a.rank()];
    for i in (0..a.rank().saturating_sub(1)).rev() {
        strides[i] = strides[i + 1]
            .checked_mul(a.shape[i + 1])
            .ok_or_else(|| err("要素が多すぎます"))?;
    }
    let mut data = Vec::with_capacity(size);
    for flat in 0..size {
        let mut remain = flat;
        let mut original = 0;
        for j in (0..shape.len()).rev() {
            let index = remain % shape[j];
            remain /= shape[j];
            original += index * strides[order[j]];
        }
        data.push(a.data.get(original).expect("permutation index"));
    }
    Ok(with_kind(a, shape, data))
}

fn transpose_axes(axes: &Arr, a: &Arr) -> Result<Arr, AzaError> {
    let labels = as_list(axes, "転")?;
    if a.rank() == 0 {
        return Err(err(
            "「転」で向きを入れ替えられるのは、リストや表です（数1つがありました）",
        ));
    }
    if labels.len() != a.rank() {
        return Err(err(format!(
            "「転」の右には、向きを{}個書きます（{}なので。{}個ありました）",
            a.rank(),
            shape_text(a),
            labels.len()
        )));
    }
    let labels = labels
        .iter()
        .map(|x| {
            whole(x, "転")
                .ok()
                .and_then(|n| n.to_usize())
                .filter(|n| *n > 0)
                .ok_or_else(|| err("「転」の向きは1以上の整数にしてください"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rank = *labels.iter().max().expect("non-scalar has axes");
    let distinct: HashSet<_> = labels.iter().copied().collect();
    if !(1..=rank).all(|n| distinct.contains(&n)) {
        return Err(err("「転」の向きは1から順につながるように指定してください"));
    }
    let mut shape = vec![usize::MAX; rank];
    for (axis, label) in labels.iter().enumerate() {
        shape[*label - 1] = shape[*label - 1].min(a.shape[axis]);
    }
    let size = shape_size(&shape)?;
    let mut strides = vec![1usize; a.rank()];
    for i in (0..a.rank().saturating_sub(1)).rev() {
        strides[i] = strides[i + 1]
            .checked_mul(a.shape[i + 1])
            .ok_or_else(|| err("要素が多すぎます"))?;
    }
    let mut data = Vec::with_capacity(size);
    for flat in 0..size {
        let output = coordinates(flat, &shape);
        let original = labels
            .iter()
            .enumerate()
            .map(|(axis, label)| output[label - 1] * strides[axis])
            .sum();
        data.push(a.data.get(original).expect("transpose index"));
    }
    Ok(with_kind(a, shape, data))
}

pub fn rotate(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let n = as_int(b, "移")?;
    if a.is_scalar() || a.shape.last() == Some(&0) {
        return Ok(a.clone());
    }
    let width = *a.shape.last().unwrap();
    let k = n.mod_floor(&BigInt::from(width)).to_usize().unwrap();
    let mut data = Vec::with_capacity(a.data.len());
    for start in (0..a.data.len()).step_by(width) {
        for column in k..width {
            data.push(a.data.get(start + column).expect("rotation index"));
        }
        for column in 0..k {
            data.push(a.data.get(start + column).expect("rotation index"));
        }
    }
    Ok(with_kind(a, a.shape.clone(), data))
}
/// 拾・替（表）の位置。範囲の外なら、何番目の向きの話かを添えて知らせる。
fn axis_index(p: &Elem, len: usize, message: impl Fn() -> String) -> Result<usize, AzaError> {
    match whole(p, "拾").ok().and_then(|n| n.to_usize()) {
        Some(n) if n >= 1 && n <= len => Ok(n - 1),
        _ => Err(err(message())),
    }
}
fn offset(a: &Arr, positions: &[Elem]) -> Result<usize, AzaError> {
    let mut result = 0;
    for (axis, p) in positions.iter().enumerate() {
        let len = a.shape[axis];
        let i = axis_index(p, len, || {
            format!(
                "{}番目はありません（{}つ目の向きは1〜{len}番目まで）",
                show_elem(p),
                axis + 1
            )
        })?;
        let stride = shape_size(&a.shape[axis + 1..])?;
        result += i * stride;
    }
    Ok(result)
}
fn pick_at(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    if a.is_scalar() {
        return Err(err("「拾」はリストや表に使います"));
    }
    let p = as_list(b, "拾")?;
    if p.is_empty() || p.len() > a.rank() {
        return Err(err(format!(
            "「拾」の右には、位置を1〜{}個書きます（{}なので）",
            a.rank(),
            shape_text(a)
        )));
    }
    let start = offset(a, &p)?;
    let shape = a.shape[p.len()..].to_vec();
    let size = shape_size(&shape)?;
    Ok(if shape.is_empty() {
        item(&a.data[start])
    } else {
        with_kind(a, shape, a.data[start..start + size].to_vec())
    })
}
fn replace_at(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    if a.is_scalar() {
        return Err(err("「替」はリストや表に使います"));
    }
    let values = as_list(b, "替")?;
    if values.len() < 2 {
        return Err(err(
            "「替」の右には、位置と新しい値を書きます（例: 替 3 100、表なら 替 2 3 0）",
        ));
    }
    let positions = &values[..values.len() - 1];
    let mut value = values.last().unwrap().clone();
    let text_like = a.kind == "text"
        || (!a.data.is_empty() && a.data.iter().all(|x| matches!(x, Elem::Char(_))));
    if text_like {
        if let Elem::Array(v) = &value {
            if is_string(v) && v.data.len() == 1 {
                value = v.data[0].clone();
            }
        }
    }
    let mut data = a.data.to_vec();
    if a.rank() == 1 {
        let len = a.shape[0];
        for p in positions {
            let i = axis_index(p, len, || {
                format!("{}番目はありません（1〜{len}番目まで）", show_elem(p))
            })?;
            data[i] = value.clone();
        }
    } else {
        if positions.len() != a.rank() {
            return Err(err(format!(
                "表の「替」には、位置を{}個（行 列）と新しい値を書きます",
                a.rank()
            )));
        }
        data[offset(a, positions)?] = value;
    }
    Ok(with_kind(a, a.shape.clone(), data))
}
fn flatten(a: &Arr) -> Result<Arr, AzaError> {
    fn count(x: &Elem) -> BigInt {
        match x {
            Elem::Array(a) => a.data.iter().map(count).sum(),
            _ => BigInt::one(),
        }
    }
    // 作る前に、全部でいくつになるかを確かめる（途中で止めると、個数の表示が変わるため）
    check_size_big(
        &(0..a.data.len())
            .map(|i| count(&a.data.get(i).expect("flatten index")))
            .sum(),
    )?;
    fn visit(x: &Elem, out: &mut Vec<Elem>) -> Result<(), AzaError> {
        match x {
            Elem::Array(a) => {
                for i in 0..a.data.len() {
                    visit(&a.data.get(i).expect("nested flatten index"), out)?;
                }
            }
            _ => out.push(x.clone()),
        }
        Ok(())
    }
    let mut data = Vec::new();
    for i in 0..a.data.len() {
        visit(&a.data.get(i).expect("flatten index"), &mut data)?;
    }
    let text = !data.is_empty() && data.iter().all(|x| matches!(x, Elem::Char(_)));
    let mut r = Arr::vector(data);
    if text {
        r.kind = "text".into();
    }
    Ok(r)
}
fn where_positions(a: &Arr) -> Result<Arr, AzaError> {
    let counts = counts(&as_list(a, "所")?, "所")?;
    let mut data = Vec::new();
    for (i, n) in counts.into_iter().enumerate() {
        data.extend(std::iter::repeat_n(Elem::int(i + 1), n));
    }
    Ok(Arr::vector(data))
}
fn as_items(a: &Arr, other: &Arr, name: &str) -> Result<Vec<Elem>, AzaError> {
    if is_string(a) && (is_nested(other) || other.data.is_empty()) {
        Ok(vec![Elem::Array(Box::new(a.clone()))])
    } else {
        as_list(a, name)
    }
}
fn unique(a: &Arr) -> Result<Arr, AzaError> {
    let mut keys = Keys(0);
    let mut seen = HashSet::new();
    let data = as_list(a, "唯")?
        .into_iter()
        .filter(|x| seen.insert(keys.of(x)))
        .collect();
    Ok(like(a, data))
}
fn set_filter(a: &Arr, b: &Arr, name: &str) -> Result<Arr, AzaError> {
    let mut keys = Keys(0);
    let right: HashSet<Key> = as_items(b, a, name)?.iter().map(|x| keys.of(x)).collect();
    let data = as_items(a, b, name)?
        .into_iter()
        .filter(|x| right.contains(&keys.of(x)) == (name == "共"))
        .collect();
    Ok(like(a, data))
}
fn group(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let values = as_list(a, "別")?;
    let keys = as_list(b, "別")?;
    if values.len() != keys.len() {
        return Err(err(format!(
            "「別」の左右の長さが合いません（{} と {}）",
            shape_text(a),
            shape_text(b)
        )));
    }
    let mut index = std::collections::HashMap::new();
    let mut groups: Vec<Vec<Elem>> = Vec::new();
    let mut hash = Keys(0);
    for (value, key) in values.into_iter().zip(keys) {
        let i = *index.entry(hash.of(&key)).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[i].push(value);
    }
    Ok(Arr::vector(
        groups
            .into_iter()
            .map(|v| Elem::Array(Box::new(like(a, v))))
            .collect(),
    ))
}

fn split(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let text = text_of(a, "分")?;
    let sep = text_of(b, "分")?;
    if sep.is_empty() {
        return Err(err(
            "「分」の右には、区切りの文字を書いてください（例: 「、」）",
        ));
    }
    let count = text.matches(&sep).count() + 1;
    check_size(count)?;
    Ok(Arr::vector(
        text.split(&sep)
            .map(|s| Elem::Array(Box::new(Arr::text(s))))
            .collect(),
    ))
}
fn piece_text(x: &Elem) -> String {
    match x {
        Elem::Array(a) if is_string(a) || a.data.is_empty() => text_of(a, "組").unwrap_or_default(),
        Elem::Array(a) => format_value(a),
        Elem::Char(c) => c.to_string(),
        _ => fmt_num(x),
    }
}
fn join(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let sep = if b.data.is_empty() {
        String::new()
    } else {
        text_of(b, "組")?
    };
    let pieces: Vec<_> = as_list(a, "組")?.iter().map(piece_text).collect();
    let size = pieces
        .iter()
        .map(|x| x.chars().count())
        .sum::<usize>()
        .checked_add(
            sep.chars()
                .count()
                .checked_mul(pieces.len().saturating_sub(1))
                .ok_or_else(|| err("要素が多すぎます"))?,
        )
        .ok_or_else(|| err("要素が多すぎます"))?;
    check_size(size)?;
    Ok(Arr::text(&pieces.join(&sep)))
}
fn parse_number(text: &str) -> Result<Elem, AzaError> {
    let normalized = text.replace('_', "");
    let bytes = text.as_bytes();
    let underscores_valid = bytes.iter().enumerate().all(|(i, c)| {
        *c != b'_'
            || (i > 0
                && bytes[i - 1].is_ascii_digit()
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
    });
    if underscores_valid {
        if let Ok(n) = normalized.parse::<BigInt>() {
            return check_integer(n);
        }
        let parsed = match normalized.to_ascii_lowercase().as_str() {
            "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
            "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
            "nan" | "+nan" | "-nan" => Some(f64::NAN),
            _ => normalized.parse::<f64>().ok(),
        };
        if let Some(n) = parsed {
            return Ok(Elem::Float(n));
        }
    }
    Err(err(format!("「{text}」は数として読めません")))
}
fn to_number(a: &Arr) -> Result<Arr, AzaError> {
    if !a.data.is_empty() && a.data.iter().all(is_number) {
        return Err(err(
            "「数」は文字を数にする字です（個数を数えるなら「個」を使います）",
        ));
    }
    if is_string(a) {
        // 区切りで分けてから1つずつ正規化する（読めないときは、書いたとおりの字を見せる）
        let text = text_of(a, "数")?;
        let nums = text
            .split(|c: char| c.is_whitespace() || matches!(c, '、' | ',' | '，' | '､'))
            .filter(|piece| !piece.is_empty())
            .map(|piece| {
                let plain = piece.nfkc().collect::<String>().replace('−', "-");
                parse_number(&plain).map_err(|e| {
                    if e.message.ends_with("は数として読めません") {
                        err(format!("「{piece}」は数として読めません"))
                    } else {
                        e
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if nums.is_empty() {
            return Err(err("空の文字は数にできません"));
        }
        return Ok(if nums.len() == 1 {
            Arr::scalar(nums[0].clone())
        } else {
            Arr::vector(nums)
        });
    }
    if is_nested(a) {
        let data = a
            .data
            .iter()
            .map(|x| match x {
                Elem::Array(a) => Ok(pack(to_number(a)?)),
                _ => Ok(x.clone()),
            })
            .collect::<Result<_, AzaError>>()?;
        return Ok(Arr::new(a.shape.clone(), data));
    }
    Err(err(format!(
        "「数」は文字を数にします（{}がありました）",
        shape_text(a)
    )))
}
fn trim(a: &Arr) -> Result<Arr, AzaError> {
    if is_string(a) {
        // 空白は、全角・半角のスペース、タブ、改行（CR も）など Unicode の空白すべて
        return Ok(Arr::text(
            text_of(a, "削")?.trim_matches(char::is_whitespace),
        ));
    }
    if is_nested(a) {
        let data = a
            .data
            .iter()
            .map(|x| match x {
                Elem::Array(v) => Ok(Elem::Array(Box::new(trim(v)?))),
                _ => Ok(x.clone()),
            })
            .collect::<Result<_, AzaError>>()?;
        return Ok(Arr::new(a.shape.clone(), data));
    }
    Err(err(format!(
        "「削」は文字に使います（{}がありました）",
        shape_text(a)
    )))
}
fn exchange(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let pairs = as_list(b, "換")?;
    if pairs.len() < 2
        || pairs.len() % 2 != 0
        || !pairs
            .iter()
            .all(|x| matches!(x,Elem::Array(v)if is_string(v)))
    {
        return Err(err(
            "「換」の右には、置き換える前と後の文字を組で並べます（例: 換 「雨」 「晴れ」）",
        ));
    }
    let texts = pairs
        .iter()
        .map(|x| {
            if let Elem::Array(v) = x {
                text_of(v, "換")
            } else {
                unreachable!()
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if texts.iter().step_by(2).any(String::is_empty) {
        return Err(err("「換」で置き換える前の文字が空です"));
    }
    let replace = |value: &Arr| -> Result<Arr, AzaError> {
        let mut text = text_of(value, "換")?;
        for pair in texts.chunks(2) {
            let count = text.matches(&pair[0]).count();
            let removed = count
                .checked_mul(pair[0].chars().count())
                .ok_or_else(|| err("要素が多すぎます"))?;
            let size = (text.chars().count() - removed)
                .checked_add(
                    count
                        .checked_mul(pair[1].chars().count())
                        .ok_or_else(|| err("要素が多すぎます"))?,
                )
                .ok_or_else(|| err("要素が多すぎます"))?;
            check_size(size)?;
            text = text.replace(&pair[0], &pair[1]);
        }
        Ok(Arr::text(&text))
    };
    if is_string(a) {
        return replace(a);
    }
    if is_nested(a) {
        let data = a
            .data
            .iter()
            .map(|x| match x {
                Elem::Array(v) => Ok(Elem::Array(Box::new(replace(v)?))),
                _ => Ok(x.clone()),
            })
            .collect::<Result<_, AzaError>>()?;
        return Ok(Arr::new(a.shape.clone(), data));
    }
    Err(err(format!(
        "「換」は文字に使います（{}がありました）",
        shape_text(a)
    )))
}
fn check(a: &Arr) -> Result<Arr, AzaError> {
    if a.data.is_empty() || a.data.iter().any(|x| boolean(x, "検").is_err()) {
        return Err(err(format!(
            "「検」の前には、比べた結果（真は1、偽は0）が必要です（{}がありました）",
            format_value(a)
        )));
    }
    if a.data.iter().any(|x| *x != Elem::int(1)) {
        let suffix = if a.rank() == 1 {
            format!(
                "（{}番目が偽）",
                a.data
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| **x != Elem::int(1))
                    .map(|(i, _)| (i + 1).to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            )
        } else {
            String::new()
        };
        return Err(err(format!("検: 真ではありませんでした{suffix}")));
    }
    Ok(a.clone())
}
/// 位で分ける数（0以上の整数）。そうでなければ言語のエラーとして知らせる。
fn splittable(x: &Elem) -> Result<BigInt, AzaError> {
    match whole(x, "位") {
        Ok(n) if !n.is_negative() => Ok(n),
        _ => Err(err(format!(
            "「位」で分けられるのは0以上の整数です（{}がありました）",
            show_elem(x)
        ))),
    }
}
fn place(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    if b.rank() > 1 || b.data.len() != 1 {
        return mixed_radix(a, b);
    }
    let base = as_int(b, "位")?;
    if base < BigInt::from(2) {
        return Err(err(format!(
            "「位」の右には2以上の整数を書きます（何進数か。{}がありました）",
            fmt_num(&Elem::int(base))
        )));
    }
    if a.is_scalar() {
        let mut n = splittable(&a.data[0])?;
        let mut data = Vec::new();
        loop {
            let (q, r) = n.div_rem(&base);
            data.push(Elem::int(r));
            n = q;
            if n.is_zero() {
                break;
            }
            check_size(data.len() + 1)?;
        }
        data.reverse();
        return Ok(Arr::vector(data));
    }
    let mut total = BigInt::zero();
    for d in as_list(a, "位")? {
        let n = whole(&d, "位")
            .ok()
            .filter(|n| !n.is_negative() && *n < base);
        let Some(n) = n else {
            return Err(err(format!(
                "{}進数の位には0〜{}の整数を並べます（{}がありました）",
                fmt_num(&Elem::int(base.clone())),
                fmt_num(&Elem::int(&base - 1)),
                show_elem(&d)
            )));
        };
        total = total * &base + n;
        if total.bits() > 33_223 {
            return Err(err("計算で数が大きすぎます（10000桁まで）"));
        }
    }
    Ok(Arr::scalar(check_integer(total)?))
}
fn mixed_radix(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let bases = as_list(b, "位")?
        .iter()
        .map(|x| {
            whole(x, "位")
                .ok()
                .filter(|n| !n.is_negative() && !n.is_one())
                .ok_or_else(|| {
                    err(format!(
                        "「位」の右には、0か2以上の整数を並べます（{}がありました）",
                        show_elem(x)
                    ))
                })
        })
        .collect::<Result<Vec<_>, AzaError>>()?;
    if a.is_scalar() {
        let mut n = splittable(&a.data[0])?;
        let mut data = Vec::with_capacity(bases.len());
        for base in bases.iter().rev() {
            let (q, r) = if base.is_zero() {
                (BigInt::zero(), n)
            } else {
                n.div_rem(base)
            };
            n = q;
            data.push(Elem::int(r));
        }
        data.reverse();
        return Ok(Arr::vector(data));
    }
    let digits = as_list(a, "位")?;
    if digits.len() != bases.len() {
        return Err(err(format!(
            "「位」の左右の長さが合いません（位が{}個、進み方が{}個）",
            digits.len(),
            bases.len()
        )));
    }
    let mut total = BigInt::zero();
    for (d, base) in digits.iter().zip(bases) {
        let n = whole(d, "位")
            .ok()
            .filter(|n| !n.is_negative() && (base.is_zero() || *n < base));
        let Some(n) = n else {
            return Err(err(if base.is_zero() {
                format!(
                    "位には0以上の整数を書きます（{}がありました）",
                    show_elem(d)
                )
            } else {
                format!(
                    "{}進の位には0〜{}の整数を書きます（{}がありました）",
                    fmt_num(&Elem::int(base.clone())),
                    fmt_num(&Elem::int(&base - 1)),
                    show_elem(d)
                )
            }));
        };
        total = if base.is_zero() { n } else { total * base + n };
        if total.bits() > 33_223 {
            return Err(err("計算で数が大きすぎます（10000桁まで）"));
        }
    }
    Ok(Arr::scalar(check_integer(total)?))
}
fn mix_split(a: &Arr) -> Result<Arr, AzaError> {
    if a.rank() >= 2 {
        check_size(a.shape[0])?;
        let width = shape_size(&a.shape[1..])?;
        return Ok(Arr::vector(
            (0..a.shape[0])
                .map(|i| {
                    pack(with_kind(
                        a,
                        a.shape[1..].to_vec(),
                        a.data[i * width..(i + 1) * width].to_vec(),
                    ))
                })
                .collect(),
        ));
    }
    if !is_nested(a) {
        return Ok(a.clone());
    }
    let rows: Vec<_> = a.data.iter().map(item).collect();
    if rows.iter().any(|r| r.rank() > 1) {
        return Err(err(
            "「編」で表にできるのは、リストを並べたものです（表を並べたものは使えません）",
        ));
    }
    let width = rows.iter().map(|r| r.data.len()).max().unwrap_or(0);
    let shape = vec![rows.len(), width];
    let size = shape_size(&shape)?;
    let text = rows.iter().all(is_string);
    let fill = if text { Elem::Char(' ') } else { Elem::int(0) };
    let mut data = Vec::with_capacity(size);
    for row in rows {
        data.extend(row.data.iter().cloned());
        data.extend(std::iter::repeat_n(fill.clone(), width - row.data.len()));
    }
    let mut result = Arr::new(shape, data);
    if text {
        result.kind = "text".into();
    }
    Ok(result)
}
fn exact(x: &Elem) -> Result<BigRational, AzaError> {
    match x {
        Elem::Int(n) => Ok(BigRational::from_integer(n.to_big())),
        Elem::Float(n) => decimal(*n).ok_or_else(|| err("「方」はこの値では計算できません")),
        _ => Err(err(format!(
            "「方」は数にしか使えません（{}がありました）",
            show_elem(x)
        ))),
    }
}
/// 小数を、書いたときの10進の数（0.1 なら 1/10）の分数にする
fn decimal(n: f64) -> Option<BigRational> {
    if !n.is_finite() {
        return None;
    }
    let text = format!("{:e}", n);
    let (mantissa, exponent) = text.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len() as i32);
    let digits: BigInt = mantissa.replace('.', "").parse().ok()?;
    let power = exponent - fraction;
    let ten = BigInt::from(10).pow(power.unsigned_abs());
    Some(if power >= 0 {
        BigRational::from_integer(digits * ten)
    } else {
        BigRational::new(digits, ten)
    })
}
fn plain(x: &BigRational) -> Result<Elem, AzaError> {
    if x.is_integer() {
        check_integer(x.to_integer())
    } else {
        x.to_f64()
            .filter(|n| n.is_finite())
            .map(Elem::Float)
            .ok_or_else(|| err("「方」の計算で数が大きすぎます"))
    }
}
fn square(a: &Arr) -> Result<usize, AzaError> {
    if a.rank() != 2 || a.shape[0] != a.shape[1] || a.shape[0] == 0 {
        return Err(err(format!(
            "「方」の左には、縦と横が同じ長さの表を書きます（{}がありました）",
            shape_text(a)
        )));
    }
    check_size(
        a.shape[0]
            .checked_pow(3)
            .ok_or_else(|| err("要素が多すぎます"))?,
    )?;
    Ok(a.shape[0])
}
/// 連立方程式を分数で正確に解く。各行を整数にしてから、分数を作らない消去法（Bareiss）で解くので、
/// 途中の数がふくらまない（100行100列でもすぐ終わる）
fn solve(a: &Arr, b: &[Vec<Elem>]) -> Result<Vec<Vec<Elem>>, AzaError> {
    let n = a.shape[0];
    let mut rows: Vec<Vec<BigInt>> = Vec::with_capacity(n);
    for (i, right) in b.iter().enumerate().take(n) {
        let mut row = (i * n..(i + 1) * n)
            .map(|k| exact(&a.data.get(k).expect("index within the array")))
            .collect::<Result<Vec<_>, _>>()?;
        row.extend(right.iter().map(exact).collect::<Result<Vec<_>, _>>()?);
        let scale = row.iter().fold(BigInt::one(), |l, x| l.lcm(x.denom()));
        rows.push(
            row.iter()
                .map(|x| x.numer() * (&scale / x.denom()))
                .collect(),
        );
    }
    let width = rows.first().map_or(0, Vec::len);
    let mut previous = BigInt::one();
    for k in 0..n {
        let pivot = (k..n).find(|i| !rows[*i][k].is_zero()).ok_or_else(|| {
            err("この表では解けません（逆にできない表です。答えが1つに決まりません）")
        })?;
        rows.swap(k, pivot);
        let pivot_row = rows[k].clone();
        for (i, row) in rows.iter_mut().enumerate() {
            if i == k {
                continue;
            }
            let factor = row[k].clone();
            for j in 0..width {
                if j == k {
                    continue;
                }
                row[j] = (&pivot_row[k] * &row[j] - &factor * &pivot_row[j]) / &previous;
            }
            row[k] = BigInt::zero();
        }
        previous = pivot_row[k].clone();
    }
    // 最後は、どの行も対角が行列式になり、右側は 行列式 × 答え になっている
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            r[n..]
                .iter()
                .map(|x| plain(&BigRational::new(x.clone(), r[i].clone())))
                .collect()
        })
        .collect()
}
fn inverse(a: &Arr) -> Result<Arr, AzaError> {
    if a.is_scalar() {
        return apply("割", &Arr::scalar(Elem::int(1)), Some(a));
    }
    let n = square(a)?;
    let identity: Vec<_> = (0..n)
        .map(|i| (0..n).map(|j| bool_elem(i == j)).collect())
        .collect();
    let rows = solve(a, &identity)?;
    Ok(Arr::new(vec![n, n], rows.into_iter().flatten().collect()))
}
fn solve_system(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let n = square(a)?;
    if b.rank() == 1 && b.shape[0] == n {
        let rows: Vec<_> = b.data.iter().map(|x| vec![x.clone()]).collect();
        return Ok(Arr::vector(
            solve(a, &rows)?.into_iter().flatten().collect(),
        ));
    }
    if b.rank() == 2 && b.shape[0] == n {
        let k = b.shape[1];
        let rows: Vec<_> = (0..n)
            .map(|i| b.data[i * k..(i + 1) * k].to_vec())
            .collect();
        return Ok(Arr::new(
            vec![n, k],
            solve(a, &rows)?.into_iter().flatten().collect(),
        ));
    }
    Err(err(format!(
        "「方」の右には、表の行の数（{n}個）と同じ長さのリストか表を書きます（{}がありました）",
        shape_text(b)
    )))
}
fn deal(a: &Arr, b: &Arr) -> Result<Arr, AzaError> {
    let n = as_int(a, "抽")?;
    let k = as_int(b, "抽")?;
    if n.is_negative() {
        return Err(err(format!(
            "「抽」の左には、0以上の整数を書きます（{}がありました）",
            fmt_num(&Elem::int(n))
        )));
    }
    if k.is_negative() || k > n {
        return Err(err(format!(
            "「抽」は、1〜{} から0〜{}個を選びます（{}個は選べません）",
            fmt_num(&Elem::int(n.clone())),
            fmt_num(&Elem::int(n.clone())),
            fmt_num(&Elem::int(k))
        )));
    }
    let k = limit_usize(&k)?;
    if let Some(values) = RANDOM_INPUTS.with(|r| r.borrow_mut().deals.pop_front()) {
        return Ok(Arr::vector(values.into_iter().map(Elem::int).collect()));
    }
    let data = RANDOM.with(|rng| {
        let mut rng = rng.borrow_mut();
        if let Some(count) = n.to_usize().filter(|n| *n <= 4 * MAX_SIZE && k > n / 3) {
            let mut all: Vec<_> = (1..=count).map(Elem::int).collect();
            all.shuffle(&mut *rng);
            all.truncate(k);
            all
        } else {
            let mut seen = HashSet::with_capacity(k);
            let mut data = Vec::with_capacity(k);
            while data.len() < k {
                let v = rng.gen_bigint_range(&BigInt::one(), &(&n + 1));
                if seen.insert(v.clone()) {
                    data.push(Elem::int(v));
                }
            }
            data
        }
    });
    Ok(Arr::vector(data))
}
fn shuffle(a: &Arr) -> Result<Arr, AzaError> {
    if a.is_scalar() {
        let n = as_int(a, "抽")?;
        if n.is_negative() {
            return Err(err(format!(
                "「抽」にはマイナスの数は使えません（{}がありました）",
                fmt_num(&Elem::int(n))
            )));
        }
        return deal(a, a);
    }
    check_size(a.shape[0])?;
    let width = shape_size(&a.shape[1..])?;
    let mut indices: Vec<_> = (0..a.shape[0]).collect();
    if let Some(values) = RANDOM_INPUTS.with(|r| r.borrow_mut().shuffles.pop_front()) {
        if values.len() != indices.len() || values.iter().any(|i| *i >= indices.len()) {
            return Err(err("並べ替えの位置が正しくありません"));
        }
        indices = values;
    } else {
        RANDOM.with(|rng| indices.shuffle(&mut *rng.borrow_mut()));
    }
    Ok(with_kind(
        a,
        a.shape.clone(),
        indices
            .into_iter()
            .flat_map(|i| a.data[i * width..(i + 1) * width].iter().cloned())
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(n: i64) -> Arr {
        Arr::scalar(Elem::int(n))
    }
    fn v(ns: &[i64]) -> Arr {
        Arr::vector(ns.iter().map(|n| Elem::int(*n)).collect())
    }
    fn monad(name: &str, a: &Arr) -> Arr {
        apply(name, a, None).unwrap()
    }
    fn dyad(name: &str, a: &Arr, b: &Arr) -> Arr {
        apply(name, a, Some(b)).unwrap()
    }

    #[test]
    fn exact_integer_arithmetic_and_remainders() {
        let a = Arr::scalar(Elem::int(BigInt::from(10u8).pow(100)));
        assert_eq!(dyad("引", &dyad("足", &a, &s(1)), &a), s(1));
        assert_eq!(dyad("余", &s(-5), &s(3)), s(1));
        assert_eq!(dyad("余", &s(5), &s(-3)), s(-1));
        assert_eq!(dyad("割", &a, &a), Arr::scalar(Elem::Float(1.0)));
        assert_eq!(dyad("係", &s(52), &s(5)), s(2598960));
    }

    #[test]
    fn shape_broadcast_and_logical_validation() {
        let matrix = Arr::new(vec![2, 2], v(&[1, 2, 3, 4]).data.to_vec());
        assert_eq!(
            dyad("足", &matrix, &v(&[10])),
            Arr::new(vec![2, 2], v(&[11, 12, 13, 14]).data.to_vec())
        );
        assert!(apply("且", &s(0), Some(&s(2))).is_err());
        assert!(apply("又", &s(1), Some(&s(2))).is_err());
        assert_eq!(dyad("位", &s(123), &v(&[10])), v(&[1, 2, 3]));
        assert_eq!(dyad("位", &s(3725), &v(&[0, 60, 60])), v(&[1, 2, 5]));
        assert_eq!(dyad("位", &v(&[1, 2, 5]), &v(&[0, 60, 60])), s(3725));
    }

    #[test]
    fn array_axes_and_empty_shapes() {
        let a = Arr::new(vec![2, 3], v(&[1, 2, 3, 4, 5, 6]).data.to_vec());
        assert_eq!(
            monad("転", &a),
            Arr::new(vec![3, 2], v(&[1, 4, 2, 5, 3, 6]).data.to_vec())
        );
        assert_eq!(
            dyad("移", &a, &s(-1)),
            Arr::new(vec![2, 3], v(&[3, 1, 2, 6, 4, 5]).data.to_vec())
        );
        assert_eq!(dyad("拾", &a, &s(2)), v(&[4, 5, 6]));
        assert_eq!(monad("転", &Arr::new(vec![2, 0], vec![])).shape, vec![0, 2]);
        let cube = Arr::new(vec![2, 2, 2], v(&[1, 2, 3, 4, 5, 6, 7, 8]).data.to_vec());
        assert_eq!(
            *monad("転", &cube).data,
            v(&[1, 5, 3, 7, 2, 6, 4, 8]).data.to_vec()
        );
    }

    #[test]
    fn japanese_text_and_nested_empty_values() {
        assert_eq!(dyad("探", &Arr::text("猫と犬"), &Arr::text("犬")), s(3));
        let split = dyad("分", &Arr::text("あ、、い"), &Arr::text("、"));
        assert_eq!(dyad("組", &split, &Arr::text("/")), Arr::text("あ//い"));
        assert_eq!(
            monad("数", &Arr::text("１２、−３ 4.5")),
            Arr::vector(vec![Elem::int(12), Elem::int(-3), Elem::Float(4.5)])
        );
        assert_eq!(monad("削", &Arr::text("　文\t")), Arr::text("文"));
        let a = Arr::new(vec![2, 2], Arr::text("甲乙丙丁").data.to_vec());
        let pos = Arr::vector(vec![
            Elem::int(2),
            Elem::int(1),
            Elem::Array(Box::new(Arr::text("字"))),
        ]);
        assert_eq!(
            *dyad("替", &a, &pos).data,
            Arr::text("甲乙字丁").data.to_vec()
        );
        let words = Arr::vector(vec![
            Elem::Array(Box::new(Arr::text(""))),
            Elem::Array(Box::new(Arr::text("文"))),
        ]);
        assert_eq!(
            *dyad("共", &words, &Arr::text("")).data,
            vec![Elem::Array(Box::new(Arr::text("")))]
        );
    }

    #[test]
    fn linear_systems_are_solved_with_exact_intermediates() {
        let a = Arr::new(vec![2, 2], v(&[2, 1, 1, -1]).data.to_vec());
        assert_eq!(dyad("方", &a, &v(&[5, 1])), v(&[2, 1]));
        let a = Arr::new(vec![2, 2], v(&[1, 2, 3, 5]).data.to_vec());
        assert_eq!(
            monad("方", &a),
            Arr::new(vec![2, 2], v(&[-5, 2, 3, -1]).data.to_vec())
        );
        assert!(apply(
            "方",
            &Arr::new(vec![2, 2], v(&[1, 2, 2, 4]).data.to_vec()),
            None
        )
        .is_err());
    }

    #[test]
    fn random_fixtures_preserve_draw_deal_and_row_shuffle_contract() {
        configure_random(42, vec![3], vec![vec![4, 1]], vec![vec![1, 0]]);
        assert_eq!(monad("乱", &s(6)), s(3));
        assert_eq!(dyad("抽", &s(5), &s(2)), v(&[4, 1]));
        let matrix = Arr::new(vec![2, 2], v(&[1, 2, 3, 4]).data.to_vec());
        assert_eq!(
            monad("抽", &matrix),
            Arr::new(vec![2, 2], v(&[3, 4, 1, 2]).data.to_vec())
        );
        for x in monad("乱", &v(&[1, 1, 1])).data.iter() {
            assert_eq!(*x, Elem::int(1));
        }
    }

    #[test]
    fn limits_fail_before_allocating_large_results() {
        assert!(apply("連", &s(1_000_001), None).is_err());
        assert!(apply("列", &s(1), Some(&v(&[1001, 1000]))).is_err());
        assert!(apply("選", &v(&[1, 2]), Some(&s(1_000_000))).is_err());
        assert!(apply("冪", &s(10), Some(&s(10_001))).is_err());
        assert!(apply("階", &s(3001), None).is_err());
    }

    #[test]
    fn rounding_and_exact_trigonometric_zero_types() {
        assert_eq!(
            monad(
                "丸",
                &Arr::vector(vec![Elem::Float(2.5), Elem::Float(-2.5)])
            ),
            v(&[3, -3])
        );
        assert_eq!(monad("正弦", &monad("円", &s(1))).to_json(), s(0).to_json());
        assert_eq!(
            dyad("丸", &Arr::scalar(Elem::Float(1.235)), &s(2)),
            Arr::scalar(Elem::Float(1.24))
        );
    }

    #[test]
    fn numeric_text_underscores_only_separate_digits() {
        assert_eq!(
            monad("数", &Arr::text("1_234 1.2_5")),
            Arr::vector(vec![Elem::int(1234), Elem::Float(1.25)])
        );
        for text in ["1_.0", "1._0", "1e_2", "n_an", "_1", "1__0"] {
            assert!(apply("数", &Arr::text(text), None).is_err(), "{text}");
        }
    }
}
