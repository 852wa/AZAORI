//! AZAORI の整数。ふだんは64ビットの整数で持ち、はみ出すときだけ大きな整数（BigInt）にする。
//!
//! 大きな整数は、小さな値でもメモリを確保するので遅い。ほとんどの計算は64ビットに収まるので、
//! 収まる間はそのまま計算し、はみ出したときだけ BigInt に切り替える（言語の結果は変わらない）。
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use std::{cmp::Ordering, fmt, str::FromStr};

/// `Big` は64ビットに収まらない値だけに使う（同じ値の表し方が1つになるので、== で比べられる）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Int {
    Small(i64),
    /// めったに使わないので、別の場所（Box）に置いて、要素1つを小さく保つ
    Big(Box<BigInt>),
}

impl Int {
    /// 大きな整数から作る。64ビットに収まれば Small にする。
    pub fn from_big(n: BigInt) -> Self {
        match n.to_i64() {
            Some(small) => Self::Small(small),
            None => Self::Big(Box::new(n)),
        }
    }
    pub fn to_big(&self) -> BigInt {
        match self {
            Self::Small(n) => BigInt::from(*n),
            Self::Big(n) => (**n).clone(),
        }
    }
    pub fn small(&self) -> Option<i64> {
        match self {
            Self::Small(n) => Some(*n),
            Self::Big(_) => None,
        }
    }
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            Self::Small(n) => Some(*n as f64),
            Self::Big(n) => n.to_f64(),
        }
    }
    pub fn to_usize(&self) -> Option<usize> {
        match self {
            Self::Small(n) => usize::try_from(*n).ok(),
            Self::Big(n) => n.to_usize(),
        }
    }
    pub fn is_zero(&self) -> bool {
        matches!(self, Self::Small(0))
    }
    pub fn is_negative(&self) -> bool {
        match self {
            Self::Small(n) => *n < 0,
            Self::Big(n) => n.is_negative(),
        }
    }
    pub fn abs(&self) -> Self {
        match self {
            Self::Small(n) if *n != i64::MIN => Self::Small(n.abs()),
            _ => Self::from_big(self.to_big().abs()),
        }
    }
}

impl PartialOrd for Int {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Int {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Small(a), Self::Small(b)) => a.cmp(b),
            _ => self.to_big().cmp(&other.to_big()),
        }
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Small(n) => write!(f, "{n}"),
            Self::Big(n) => write!(f, "{n}"),
        }
    }
}

impl FromStr for Int {
    type Err = num_bigint::ParseBigIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse::<i64>() {
            Ok(n) => Ok(Self::Small(n)),
            Err(_) => s.parse::<BigInt>().map(Self::from_big),
        }
    }
}

/// 整数にできるもの（Elem::int に渡せる型）
pub trait IntoInt {
    fn into_int(self) -> Int;
}

macro_rules! small_ints {
    ($($t:ty),*) => {$(
        impl IntoInt for $t {
            fn into_int(self) -> Int {
                match i64::try_from(self) {
                    Ok(n) => Int::Small(n),
                    Err(_) => Int::from_big(BigInt::from(self)),
                }
            }
        }
    )*};
}
small_ints!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

impl IntoInt for BigInt {
    fn into_int(self) -> Int {
        Int::from_big(self)
    }
}
impl IntoInt for &BigInt {
    fn into_int(self) -> Int {
        match self.to_i64() {
            Some(n) => Int::Small(n),
            None => Int::Big(Box::new(self.clone())),
        }
    }
}
impl IntoInt for Int {
    fn into_int(self) -> Int {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_traits::Zero;

    #[test]
    fn small_and_big_have_one_form() {
        let big: BigInt = "9223372036854775808".parse().unwrap(); // i64 の最大 + 1
        assert!(matches!(Int::from_big(big.clone()), Int::Big(_)));
        assert_eq!(Int::from_big(big - 1), Int::Small(i64::MAX));
        assert_eq!("-5".parse::<Int>().unwrap(), Int::Small(-5));
        assert!(Int::Small(3) < Int::from_big("99999999999999999999".parse().unwrap()));
        assert_eq!(
            Int::Small(i64::MIN).abs().to_string(),
            "9223372036854775808"
        );
        assert!(Int::Small(0).is_zero() && !BigInt::zero().is_negative());
    }
}
