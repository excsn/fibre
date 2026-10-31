/// Defines metrics. Use it at module level.
///
/// Each item is a `static` whose type names the handle: `Counter`, `Gauge`,
/// `Timer` or `State`. The string is the metric name. The doc comment is the
/// description. Attributes add facts:
///
/// * `#[unit(bytes)]`: `none`, `count`, `seconds`, `bytes`, `percent`, `ratio`, `celsius`, `hertz` or a string for anything else. A counter defaults to `count` and a timer to `seconds`.
/// * `#[tags("status", "route")]`: the tag names a recording may carry. In a debug build a recording with another tag panics.
/// * `#[high_res("1ms")]`: keep exact samples at this resolution when the `high_res` feature is on.
///
/// ```
/// use fibre_stats::{metric, tags};
///
/// metric! {
///   /// Requests answered.
///   #[tags("status")]
///   pub static REQUESTS: Counter = "http.requests";
///
///   /// Time to answer one request.
///   #[tags("route")]
///   pub static LATENCY: Timer = "http.latency";
/// }
///
/// REQUESTS.incr(tags!("status" => "ok"));
/// LATENCY.record(std::time::Duration::from_millis(3), tags!("route" => "/"));
/// assert!(fibre_stats::registry().any(|m| m.name == "http.requests"));
/// ```
#[macro_export]
macro_rules! metric {
  ($(
    $(#[$($attr:tt)*])*
    $vis:vis static $NAME:ident : $Handle:ident = $metric_name:literal;
  )+) => {
    $(
      $crate::__metric_one! {
        @docs [] @attrs [] [$(#[$($attr)*])*] $vis static $NAME : $Handle = $metric_name
      }
    )+
  };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __metric_one {
  (@docs [$($d:tt)*] @attrs [$($a:tt)*] [#[doc = $doc:literal] $($rest:tt)*] $($tail:tt)*) => {
    $crate::__metric_one! { @docs [$($d)* $doc,] @attrs [$($a)*] [$($rest)*] $($tail)* }
  };
  (@docs [$($d:tt)*] @attrs [$($a:tt)*] [#[unit($unit:tt)] $($rest:tt)*] $($tail:tt)*) => {
    $crate::__metric_one! {
      @docs [$($d)*]
      @attrs [$($a)* $crate::MetricAttr::Unit($crate::__metric_unit!($unit)),]
      [$($rest)*] $($tail)*
    }
  };
  (@docs [$($d:tt)*] @attrs [$($a:tt)*] [#[tags($($tag:literal),* $(,)?)] $($rest:tt)*] $($tail:tt)*) => {
    $crate::__metric_one! {
      @docs [$($d)*]
      @attrs [$($a)* $crate::MetricAttr::Tags(&[$($tag),*]),]
      [$($rest)*] $($tail)*
    }
  };
  (@docs [$($d:tt)*] @attrs [$($a:tt)*] [#[high_res($res:literal)] $($rest:tt)*] $($tail:tt)*) => {
    $crate::__metric_one! {
      @docs [$($d)*]
      @attrs [$($a)* $crate::MetricAttr::Resolution($crate::metric::parse_duration($res)),]
      [$($rest)*] $($tail)*
    }
  };
  (@docs [$($d:tt)*] @attrs [$($a:tt)*] [] $vis:vis static $NAME:ident : $Handle:ident = $metric_name:literal) => {
    $vis static $NAME: $crate::$Handle = $crate::$Handle::new($crate::Metric::new(
      $metric_name,
      $crate::Kind::$Handle,
      &[$($d)*],
      &[$($a)*],
    ));

    $crate::__inventory::submit! {
      $crate::metric::Registered(&$NAME.metric)
    }
  };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __metric_unit {
  (none) => {
    $crate::Unit::None
  };
  (count) => {
    $crate::Unit::Count
  };
  (seconds) => {
    $crate::Unit::Seconds
  };
  (bytes) => {
    $crate::Unit::Bytes
  };
  (percent) => {
    $crate::Unit::Percent
  };
  (ratio) => {
    $crate::Unit::Ratio
  };
  (celsius) => {
    $crate::Unit::Celsius
  };
  (hertz) => {
    $crate::Unit::Hertz
  };
  ($custom:literal) => {
    $crate::Unit::Custom($custom)
  };
}

/// Defines an enum that is reported as a state.
///
/// Each variant names its label. The label is what a store keys the variant
/// by, so renaming a variant does not change the series as long as the label
/// stays.
///
/// ```
/// use fibre_stats::{state_enum, EnumState, StateValue};
///
/// state_enum! {
///   /// Where a node is in its life.
///   pub enum NodeStatus {
///     Starting = "starting",
///     Serving = "serving",
///     Draining = "draining",
///   }
/// }
///
/// assert_eq!(NodeStatus::Serving.label(), "serving");
/// assert_eq!(StateValue::from(NodeStatus::Serving), StateValue::label("NodeStatus", "serving"));
/// ```
#[macro_export]
macro_rules! state_enum {
  (
    $(#[$meta:meta])*
    $vis:vis enum $Name:ident {
      $($(#[$vmeta:meta])* $Variant:ident = $label:literal),+ $(,)?
    }
  ) => {
    $(#[$meta])*
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    $vis enum $Name {
      $($(#[$vmeta])* $Variant,)+
    }

    impl $crate::EnumState for $Name {
      const ENUM_NAME: &'static str = stringify!($Name);

      fn label(&self) -> &'static str {
        match self {
          $($Name::$Variant => $label,)+
        }
      }
    }

    impl From<$Name> for $crate::StateValue {
      fn from(v: $Name) -> Self {
        $crate::EnumState::state(&v)
      }
    }

    impl From<$Name> for $crate::StatValue {
      fn from(v: $Name) -> Self {
        $crate::StatValue::State($crate::EnumState::state(&v))
      }
    }
  };
}
