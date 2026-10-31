//! Metric definitions and the handles that record through them.
//!
//! A definition is written once with [`metric!`](crate::metric!) and holds the
//! name, kind, unit, description and allowed tags. The handle it produces is
//! the only way to record that metric, so the facts and the recording cannot
//! drift apart. Every definition also registers itself, so [`registry`] lists it
//! without anything being called.

use std::time::Duration;

use crate::collect::{PollGauge, PollState, PollTotal};
use crate::recorder::{Observation, Recorder, Sample, global};
use crate::tag::{Name, Tags};
use crate::time::Stopwatch;
use crate::value::{NumberValue, StateValue};

/// What a metric measures. Decides which samples a series accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
  /// A delta, summed over a window.
  Counter,
  /// A level at a moment. The last one in a window is kept.
  Gauge,
  /// A duration, kept as a histogram per window.
  Timer,
  /// A boolean, a character or an enum label. The last one in a window is kept.
  State,
}

impl Kind {
  pub fn as_str(&self) -> &'static str {
    match self {
      Kind::Counter => "counter",
      Kind::Gauge => "gauge",
      Kind::Timer => "timer",
      Kind::State => "state",
    }
  }
}

impl std::fmt::Display for Kind {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

/// The unit of a metric's values. A counter is `Count` and a timer is
/// `Seconds` unless the definition says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
  None,
  Count,
  Seconds,
  Bytes,
  Percent,
  Ratio,
  Celsius,
  Hertz,
  Custom(&'static str),
}

impl Unit {
  pub fn as_str(&self) -> &'static str {
    match self {
      Unit::None => "",
      Unit::Count => "count",
      Unit::Seconds => "seconds",
      Unit::Bytes => "bytes",
      Unit::Percent => "percent",
      Unit::Ratio => "ratio",
      Unit::Celsius => "celsius",
      Unit::Hertz => "hertz",
      Unit::Custom(s) => s,
    }
  }
}

/// One attribute from a `metric!` definition.
#[derive(Debug, Clone, Copy)]
pub enum MetricAttr {
  Unit(Unit),
  Tags(&'static [&'static str]),
  Resolution(Duration),
}

/// The facts about one metric.
#[derive(Debug)]
pub struct Metric {
  pub name: &'static str,
  pub kind: Kind,
  pub unit: Unit,
  /// The doc comment lines, as written.
  pub description: &'static [&'static str],
  /// The tag names a recording may carry.
  pub tags: &'static [&'static str],
  /// The resolution exact samples are kept at, from `#[high_res("...")]`.
  pub resolution: Option<Duration>,
}

impl Metric {
  pub const fn new(
    name: &'static str,
    kind: Kind,
    description: &'static [&'static str],
    attrs: &'static [MetricAttr],
  ) -> Metric {
    let mut unit = match kind {
      Kind::Counter => Unit::Count,
      Kind::Timer => Unit::Seconds,
      Kind::Gauge | Kind::State => Unit::None,
    };
    let mut tags: &'static [&'static str] = &[];
    let mut resolution: Option<Duration> = None;
    let mut i = 0;
    while i < attrs.len() {
      match attrs[i] {
        MetricAttr::Unit(u) => unit = u,
        MetricAttr::Tags(t) => tags = t,
        MetricAttr::Resolution(r) => resolution = Some(r),
      }
      i += 1;
    }
    Metric {
      name,
      kind,
      unit,
      description,
      tags,
      resolution,
    }
  }

  /// The description with each line trimmed and joined by newlines.
  pub fn description_text(&self) -> String {
    let mut out = String::new();
    for line in self.description {
      let line = line.trim();
      if !out.is_empty() {
        out.push('\n');
      }
      out.push_str(line);
    }
    out
  }

  pub fn allows(&self, tag: &str) -> bool {
    self.tags.contains(&tag)
  }

  fn check_tags(&self, tags: &Tags) {
    if cfg!(debug_assertions) {
      for tag in tags {
        assert!(
          self.allows(&tag.name),
          "metric `{}` does not declare tag `{}`; add it to #[tags(...)]",
          self.name,
          tag.name
        );
      }
    }
  }

  fn recording_resolution(&self) -> Option<Duration> {
    if cfg!(feature = "high_res") { self.resolution } else { None }
  }

  fn observation(&self, recorder: &dyn Recorder, tags: Tags, sample: Sample) -> Observation {
    self.check_tags(&tags);
    Observation {
      name: Name::Borrowed(self.name),
      kind: self.kind,
      tags,
      at_ns: recorder.now_ns(),
      sample,
      resolution: self.recording_resolution(),
    }
  }
}

/// Parses `"1ms"`, `"250us"`, `"10ns"` or `"1s"` at compile time. Panics on
/// anything else, which fails the build.
pub const fn parse_duration(s: &str) -> Duration {
  let b = s.as_bytes();
  let mut i = 0;
  let mut n: u64 = 0;
  while i < b.len() && b[i] >= b'0' && b[i] <= b'9' {
    n = n * 10 + (b[i] - b'0') as u64;
    i += 1;
  }
  if i == 0 {
    panic!("a duration starts with a number: \"1ms\", \"250us\", \"10ns\" or \"1s\"");
  }
  let rest = b.len() - i;
  if rest == 2 && b[i] == b'n' && b[i + 1] == b's' {
    return Duration::from_nanos(n);
  }
  if rest == 2 && b[i] == b'u' && b[i + 1] == b's' {
    return Duration::from_micros(n);
  }
  if rest == 2 && b[i] == b'm' && b[i + 1] == b's' {
    return Duration::from_millis(n);
  }
  if rest == 1 && b[i] == b's' {
    return Duration::from_secs(n);
  }
  panic!("a duration ends in ns, us, ms or s: \"1ms\", \"250us\", \"10ns\" or \"1s\"");
}

#[doc(hidden)]
pub struct Registered(pub &'static Metric);

inventory::collect!(Registered);

/// Every `metric!` definition the linker kept, in no defined order. A definition
/// whose handle the program refers to is always kept. One in a library the
/// program does not refer to can be missing, more often on wasm than on native
/// targets. Two crates may define the same name; both appear.
pub fn registry() -> impl Iterator<Item = &'static Metric> {
  inventory::iter::<Registered>.into_iter().map(|registered| registered.0)
}

/// The first definition from [`registry`] with this name.
pub fn find(name: &str) -> Option<&'static Metric> {
  registry().find(|m| m.name == name)
}

/// The handle `metric!` produces for a `Counter`.
pub struct Counter {
  pub metric: Metric,
}

impl Counter {
  pub const fn new(metric: Metric) -> Self {
    Counter { metric }
  }

  /// Adds one on the global recorder.
  pub fn incr(&self, tags: Tags) {
    self.add_to(global(), 1, tags);
  }

  /// Adds `delta` on the global recorder.
  pub fn add(&self, delta: u64, tags: Tags) {
    self.add_to(global(), delta, tags);
  }

  pub fn add_to(&self, recorder: &dyn Recorder, delta: u64, tags: Tags) {
    let observation = self.metric.observation(recorder, tags, Sample::Count(delta));
    recorder.record(observation);
  }

  /// Reports a running total on the global recorder. The recorder turns it into
  /// deltas; see [`Recorder::total`].
  pub fn total(&self, total: u64, tags: Tags) {
    self.total_to(global(), total, tags);
  }

  pub fn total_to(&self, recorder: &dyn Recorder, total: u64, tags: Tags) {
    self.metric.check_tags(&tags);
    recorder.total(Name::Borrowed(self.metric.name), tags, total);
  }

  /// A collector that reads a running total each time it runs.
  pub fn poll_total<F>(&'static self, tags: Tags, read: F) -> PollTotal<F>
  where
    F: Fn() -> u64 + Send + 'static,
  {
    self.metric.check_tags(&tags);
    PollTotal::new(Name::Borrowed(self.metric.name), tags, read)
  }
}

/// The handle `metric!` produces for a `Gauge`.
pub struct Gauge {
  pub metric: Metric,
}

impl Gauge {
  pub const fn new(metric: Metric) -> Self {
    Gauge { metric }
  }

  /// Sets the level on the global recorder.
  pub fn set(&self, value: impl Into<NumberValue>, tags: Tags) {
    self.set_to(global(), value, tags);
  }

  pub fn set_to(&self, recorder: &dyn Recorder, value: impl Into<NumberValue>, tags: Tags) {
    let observation = self
      .metric
      .observation(recorder, tags, Sample::Number(value.into()));
    recorder.record(observation);
  }

  /// A collector that reads the level each time it runs.
  pub fn poll<F>(&'static self, tags: Tags, read: F) -> PollGauge<F>
  where
    F: Fn() -> NumberValue + Send + 'static,
  {
    self.metric.check_tags(&tags);
    PollGauge::new(Name::Borrowed(self.metric.name), tags, read)
  }
}

/// The handle `metric!` produces for a `Timer`.
pub struct Timer {
  pub metric: Metric,
}

impl Timer {
  pub const fn new(metric: Metric) -> Self {
    Timer { metric }
  }

  /// Records a duration on the global recorder.
  pub fn record(&self, duration: Duration, tags: Tags) {
    self.record_to(global(), duration, tags);
  }

  pub fn record_to(&self, recorder: &dyn Recorder, duration: Duration, tags: Tags) {
    let observation = self
      .metric
      .observation(recorder, tags, Sample::Duration(duration));
    recorder.record(observation);
  }

  /// Runs `f` and records how long it took on the global recorder.
  pub fn time<R>(&self, tags: Tags, f: impl FnOnce() -> R) -> R {
    self.time_to(global(), tags, f)
  }

  pub fn time_to<R>(&self, recorder: &dyn Recorder, tags: Tags, f: impl FnOnce() -> R) -> R {
    let start = Stopwatch::start(recorder);
    let result = f();
    self.record_to(recorder, start.elapsed(recorder), tags);
    result
  }

  /// Starts timing. The guard records on drop unless it is discarded.
  pub fn guard(&self, tags: Tags) -> TimerGuard<'_> {
    self.guard_to(global(), tags)
  }

  pub fn guard_to<'a>(&'a self, recorder: &'a dyn Recorder, tags: Tags) -> TimerGuard<'a> {
    TimerGuard {
      timer: self,
      recorder,
      tags: Some(tags),
      start: Stopwatch::start(recorder),
    }
  }
}

/// A running timer. Dropping it records the elapsed time.
pub struct TimerGuard<'a> {
  timer: &'a Timer,
  recorder: &'a dyn Recorder,
  tags: Option<Tags>,
  start: Stopwatch,
}

impl TimerGuard<'_> {
  /// Records now rather than at the end of the scope.
  pub fn stop(self) {}

  /// Drops the guard without recording.
  pub fn discard(mut self) {
    self.tags = None;
  }

  /// Replaces the tags that will be recorded, for an outcome known only at the end.
  pub fn set_tags(&mut self, tags: Tags) {
    self.tags = Some(tags);
  }

  pub fn elapsed(&self) -> Duration {
    self.start.elapsed(self.recorder)
  }
}

impl Drop for TimerGuard<'_> {
  fn drop(&mut self) {
    if let Some(tags) = self.tags.take() {
      self.timer.record_to(self.recorder, self.start.elapsed(self.recorder), tags);
    }
  }
}

/// The handle `metric!` produces for a `State`.
pub struct State {
  pub metric: Metric,
}

impl State {
  pub const fn new(metric: Metric) -> Self {
    State { metric }
  }

  /// Sets the state on the global recorder.
  pub fn set(&self, value: impl Into<StateValue>, tags: Tags) {
    self.set_to(global(), value, tags);
  }

  pub fn set_to(&self, recorder: &dyn Recorder, value: impl Into<StateValue>, tags: Tags) {
    let observation = self
      .metric
      .observation(recorder, tags, Sample::State(value.into()));
    recorder.record(observation);
  }

  /// A collector that reads the state each time it runs.
  pub fn poll<F>(&'static self, tags: Tags, read: F) -> PollState<F>
  where
    F: Fn() -> StateValue + Send + 'static,
  {
    self.metric.check_tags(&tags);
    PollState::new(Name::Borrowed(self.metric.name), tags, read)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_durations() {
    assert_eq!(parse_duration("1ms"), Duration::from_millis(1));
    assert_eq!(parse_duration("250us"), Duration::from_micros(250));
    assert_eq!(parse_duration("10ns"), Duration::from_nanos(10));
    assert_eq!(parse_duration("2s"), Duration::from_secs(2));
  }

  #[test]
  fn defaults_by_kind() {
    const NONE: &[MetricAttr] = &[];
    assert_eq!(Metric::new("a", Kind::Counter, &[], NONE).unit, Unit::Count);
    assert_eq!(Metric::new("a", Kind::Timer, &[], NONE).unit, Unit::Seconds);
    assert_eq!(Metric::new("a", Kind::Gauge, &[], NONE).unit, Unit::None);
    assert_eq!(Metric::new("a", Kind::State, &[], NONE).unit, Unit::None);
  }

  #[test]
  fn attributes_apply() {
    const ATTRS: &[MetricAttr] = &[
      MetricAttr::Unit(Unit::Bytes),
      MetricAttr::Tags(&["disk", "mount"]),
      MetricAttr::Resolution(Duration::from_millis(1)),
    ];
    let m = Metric::new("host.disk.free_bytes", Kind::Gauge, &[" Free space.", " Per mount."], ATTRS);
    assert_eq!(m.unit, Unit::Bytes);
    assert!(m.allows("disk"));
    assert!(!m.allows("cpu"));
    assert_eq!(m.resolution, Some(Duration::from_millis(1)));
    assert_eq!(m.description_text(), "Free space.\nPer mount.");
  }
}
