//! The values a metric can hold.
//!
//! A number goes to a gauge, a count to a counter, a duration to a timer and a
//! state to a state metric. Strings are not values: a label is an enum with a
//! name, so a store can map it to a number and a reader gets the label back.

use std::fmt;

/// A percentage from 0 to just under 128, with 9 fractional bits.
pub type Percent = fixed::FixedU16<fixed::types::extra::U9>;

/// A number from 0.0 to 1.0, with 31 fractional bits.
pub type Normalized = fixed::FixedU32<fixed::types::extra::U31>;

/// A ratio of two `i32`s.
pub type Ratio32 = num_rational::Rational32;

/// An integer of any width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntValue {
  I8(i8),
  U8(u8),
  I16(i16),
  U16(u16),
  I32(i32),
  U32(u32),
  I64(i64),
  U64(u64),
  I128(i128),
  U128(u128),
}

impl IntValue {
  /// The width of the integer in bits.
  pub fn bits(&self) -> u8 {
    match self {
      IntValue::I8(_) | IntValue::U8(_) => 8,
      IntValue::I16(_) | IntValue::U16(_) => 16,
      IntValue::I32(_) | IntValue::U32(_) => 32,
      IntValue::I64(_) | IntValue::U64(_) => 64,
      IntValue::I128(_) | IntValue::U128(_) => 128,
    }
  }

  pub fn is_signed(&self) -> bool {
    matches!(
      self,
      IntValue::I8(_) | IntValue::I16(_) | IntValue::I32(_) | IntValue::I64(_) | IntValue::I128(_)
    )
  }

  /// The value as an `i64`. `None` when it does not fit.
  pub fn to_i64(&self) -> Option<i64> {
    match *self {
      IntValue::I8(v) => Some(v as i64),
      IntValue::U8(v) => Some(v as i64),
      IntValue::I16(v) => Some(v as i64),
      IntValue::U16(v) => Some(v as i64),
      IntValue::I32(v) => Some(v as i64),
      IntValue::U32(v) => Some(v as i64),
      IntValue::I64(v) => Some(v),
      IntValue::U64(v) => i64::try_from(v).ok(),
      IntValue::I128(v) => i64::try_from(v).ok(),
      IntValue::U128(v) => i64::try_from(v).ok(),
    }
  }

  /// The value as a `u64`. `None` when it is negative or does not fit.
  pub fn to_u64(&self) -> Option<u64> {
    match *self {
      IntValue::I8(v) => u64::try_from(v).ok(),
      IntValue::U8(v) => Some(v as u64),
      IntValue::I16(v) => u64::try_from(v).ok(),
      IntValue::U16(v) => Some(v as u64),
      IntValue::I32(v) => u64::try_from(v).ok(),
      IntValue::U32(v) => Some(v as u64),
      IntValue::I64(v) => u64::try_from(v).ok(),
      IntValue::U64(v) => Some(v),
      IntValue::I128(v) => u64::try_from(v).ok(),
      IntValue::U128(v) => u64::try_from(v).ok(),
    }
  }

  /// The value as an `i128`. `None` for a `u128` above `i128::MAX`.
  pub fn to_i128(&self) -> Option<i128> {
    match *self {
      IntValue::I8(v) => Some(v as i128),
      IntValue::U8(v) => Some(v as i128),
      IntValue::I16(v) => Some(v as i128),
      IntValue::U16(v) => Some(v as i128),
      IntValue::I32(v) => Some(v as i128),
      IntValue::U32(v) => Some(v as i128),
      IntValue::I64(v) => Some(v as i128),
      IntValue::U64(v) => Some(v as i128),
      IntValue::I128(v) => Some(v),
      IntValue::U128(v) => i128::try_from(v).ok(),
    }
  }
}

macro_rules! int_from {
  ($($t:ty => $variant:ident),* $(,)?) => {
    $(
      impl From<$t> for IntValue {
        fn from(v: $t) -> Self {
          IntValue::$variant(v)
        }
      }

      impl From<$t> for NumberValue {
        fn from(v: $t) -> Self {
          NumberValue::Int(IntValue::$variant(v))
        }
      }

      impl From<$t> for StatValue {
        fn from(v: $t) -> Self {
          StatValue::Number(NumberValue::Int(IntValue::$variant(v)))
        }
      }
    )*
  };
}

int_from! {
  i8 => I8, u8 => U8, i16 => I16, u16 => U16, i32 => I32, u32 => U32,
  i64 => I64, u64 => U64, i128 => I128, u128 => U128,
}

impl From<usize> for IntValue {
  fn from(v: usize) -> Self {
    IntValue::U64(v as u64)
  }
}

impl From<usize> for NumberValue {
  fn from(v: usize) -> Self {
    NumberValue::Int(IntValue::U64(v as u64))
  }
}

impl fmt::Display for IntValue {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      IntValue::I8(v) => write!(f, "{v}"),
      IntValue::U8(v) => write!(f, "{v}"),
      IntValue::I16(v) => write!(f, "{v}"),
      IntValue::U16(v) => write!(f, "{v}"),
      IntValue::I32(v) => write!(f, "{v}"),
      IntValue::U32(v) => write!(f, "{v}"),
      IntValue::I64(v) => write!(f, "{v}"),
      IntValue::U64(v) => write!(f, "{v}"),
      IntValue::I128(v) => write!(f, "{v}"),
      IntValue::U128(v) => write!(f, "{v}"),
    }
  }
}

/// A number: what a gauge holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumberValue {
  Int(IntValue),
  Count(u64),
  Percent(Percent),
  Normalized(Normalized),
  Ratio32(Ratio32),
}

impl NumberValue {
  pub fn count(v: u64) -> Self {
    NumberValue::Count(v)
  }

  /// A percentage. `None` when `v` is NaN, negative or at least 128.
  pub fn percent(v: f32) -> Option<Self> {
    if v.is_nan() || v < 0.0 {
      return None;
    }
    Percent::checked_from_num(v).map(NumberValue::Percent)
  }

  /// A percentage clamped into what `Percent` holds. NaN becomes zero.
  pub fn percent_saturating(v: f32) -> Self {
    if v.is_nan() {
      return NumberValue::Percent(Percent::ZERO);
    }
    NumberValue::Percent(Percent::saturating_from_num(v.max(0.0)))
  }

  /// A number in `[0, 1]`. `None` when `v` is NaN or outside it.
  pub fn normalized(v: f64) -> Option<Self> {
    if v.is_nan() || !(0.0..=1.0).contains(&v) {
      return None;
    }
    Normalized::checked_from_num(v).map(NumberValue::Normalized)
  }

  /// The closest ratio of two `i32`s to `v`. `None` when there is none.
  pub fn ratio(v: f64) -> Option<Self> {
    Ratio32::approximate_float(v).map(NumberValue::Ratio32)
  }

  /// A ratio from a numerator and a denominator. Panics when `denom` is zero.
  pub fn ratio_of(numer: i32, denom: i32) -> Self {
    NumberValue::Ratio32(Ratio32::new(numer, denom))
  }

  pub fn encoding(&self) -> Encoding {
    match self {
      NumberValue::Int(_) => Encoding::Int,
      NumberValue::Count(_) => Encoding::Count,
      NumberValue::Percent(_) => Encoding::Percent,
      NumberValue::Normalized(_) => Encoding::Normalized,
      NumberValue::Ratio32(_) => Encoding::Ratio32,
    }
  }

  /// The value as an `f64`, for display and for tests.
  pub fn to_f64(&self) -> f64 {
    match self {
      NumberValue::Int(v) => v.to_i128().map(|i| i as f64).unwrap_or(f64::INFINITY),
      NumberValue::Count(v) => *v as f64,
      NumberValue::Percent(v) => v.to_num(),
      NumberValue::Normalized(v) => v.to_num(),
      NumberValue::Ratio32(v) => *v.numer() as f64 / *v.denom() as f64,
    }
  }
}

impl From<IntValue> for NumberValue {
  fn from(v: IntValue) -> Self {
    NumberValue::Int(v)
  }
}

impl From<Percent> for NumberValue {
  fn from(v: Percent) -> Self {
    NumberValue::Percent(v)
  }
}

impl From<Normalized> for NumberValue {
  fn from(v: Normalized) -> Self {
    NumberValue::Normalized(v)
  }
}

impl From<Ratio32> for NumberValue {
  fn from(v: Ratio32) -> Self {
    NumberValue::Ratio32(v)
  }
}

impl fmt::Display for NumberValue {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      NumberValue::Int(v) => write!(f, "{v}"),
      NumberValue::Count(v) => write!(f, "{v}"),
      NumberValue::Percent(v) => write!(f, "{v}"),
      NumberValue::Normalized(v) => write!(f, "{v}"),
      NumberValue::Ratio32(v) => write!(f, "{}/{}", v.numer(), v.denom()),
    }
  }
}

/// A state: what a state metric holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateValue {
  Bool(bool),
  Char(char),
  Enum {
    enum_name: &'static str,
    label: &'static str,
  },
}

impl StateValue {
  pub fn label(enum_name: &'static str, label: &'static str) -> Self {
    StateValue::Enum { enum_name, label }
  }
}

impl From<bool> for StateValue {
  fn from(v: bool) -> Self {
    StateValue::Bool(v)
  }
}

impl From<char> for StateValue {
  fn from(v: char) -> Self {
    StateValue::Char(v)
  }
}

impl fmt::Display for StateValue {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      StateValue::Bool(v) => write!(f, "{v}"),
      StateValue::Char(v) => write!(f, "{v}"),
      StateValue::Enum { enum_name, label } => write!(f, "{enum_name}::{label}"),
    }
  }
}

/// A Rust enum that is reported as a state.
///
/// `state_enum!` writes the implementation. A hand-written one returns a label
/// that never changes for a variant, since a store keys the variant by it.
pub trait EnumState: Copy {
  const ENUM_NAME: &'static str;

  fn label(&self) -> &'static str;

  fn state(&self) -> StateValue {
    StateValue::Enum {
      enum_name: Self::ENUM_NAME,
      label: self.label(),
    }
  }
}

/// A number or a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatValue {
  Number(NumberValue),
  State(StateValue),
}

impl StatValue {
  pub fn encoding(&self) -> Encoding {
    match self {
      StatValue::Number(n) => n.encoding(),
      StatValue::State(_) => Encoding::State,
    }
  }
}

impl From<NumberValue> for StatValue {
  fn from(v: NumberValue) -> Self {
    StatValue::Number(v)
  }
}

impl From<StateValue> for StatValue {
  fn from(v: StateValue) -> Self {
    StatValue::State(v)
  }
}

impl From<bool> for StatValue {
  fn from(v: bool) -> Self {
    StatValue::State(StateValue::Bool(v))
  }
}

impl fmt::Display for StatValue {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      StatValue::Number(v) => write!(f, "{v}"),
      StatValue::State(v) => write!(f, "{v}"),
    }
  }
}

/// How a series' points are encoded. Fixed on a series' first point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
  Int,
  Count,
  Percent,
  Normalized,
  Ratio32,
  State,
  Histogram,
}

impl Encoding {
  pub fn as_str(&self) -> &'static str {
    match self {
      Encoding::Int => "int",
      Encoding::Count => "count",
      Encoding::Percent => "percent",
      Encoding::Normalized => "normalized",
      Encoding::Ratio32 => "ratio32",
      Encoding::State => "state",
      Encoding::Histogram => "histogram",
    }
  }
}

impl fmt::Display for Encoding {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn integer_conversions() {
    assert_eq!(IntValue::from(7u8).bits(), 8);
    assert_eq!(IntValue::from(-1i32).to_u64(), None);
    assert_eq!(IntValue::from(u64::MAX).to_i64(), None);
    assert_eq!(IntValue::from(u128::MAX).to_i128(), None);
    assert_eq!(IntValue::from(300u16).to_i64(), Some(300));
    assert!(IntValue::from(-5i8).is_signed());
    assert!(!IntValue::from(5u8).is_signed());
  }

  #[test]
  fn percent_bounds() {
    assert_eq!(NumberValue::percent(f32::NAN), None);
    assert_eq!(NumberValue::percent(-1.0), None);
    assert_eq!(NumberValue::percent(200.0), None);
    assert!((NumberValue::percent(12.5).unwrap().to_f64() - 12.5).abs() < 1e-6);
    assert_eq!(NumberValue::percent_saturating(f32::NAN).to_f64(), 0.0);
    assert!(NumberValue::percent_saturating(500.0).to_f64() > 127.0);
  }

  #[test]
  fn normalized_and_ratio() {
    assert_eq!(NumberValue::normalized(1.5), None);
    assert_eq!(NumberValue::normalized(1.0).unwrap().to_f64(), 1.0);
    assert!((NumberValue::normalized(0.25).unwrap().to_f64() - 0.25).abs() < 1e-9);
    let r = NumberValue::ratio(0.15).unwrap();
    assert!((r.to_f64() - 0.15).abs() < 1e-6);
    assert_eq!(NumberValue::ratio_of(3, 4).to_f64(), 0.75);
  }

  #[test]
  fn encodings() {
    assert_eq!(NumberValue::from(1i64).encoding(), Encoding::Int);
    assert_eq!(NumberValue::count(1).encoding(), Encoding::Count);
    assert_eq!(StatValue::from(true).encoding(), Encoding::State);
    assert_eq!(StateValue::label("Node", "up").to_string(), "Node::up");
  }
}
