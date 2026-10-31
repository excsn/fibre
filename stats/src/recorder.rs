//! Where recordings go.
//!
//! A [`Recorder`] takes observations. [`Stats`](crate::Stats) is the one that
//! aggregates them; [`Context`] adds tags on the way to another recorder and
//! [`Noop`] drops everything. One recorder can be made global, which is what
//! the handles from `metric!` use when no recorder is passed.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::error::SetGlobalError;
use crate::metric::Kind;
use crate::tag::{Name, Tags};
use crate::value::{Encoding, NumberValue, StateValue};

/// One recorded value, before aggregation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sample {
  /// A counter delta.
  Count(u64),
  /// A gauge level.
  Number(NumberValue),
  /// A state.
  State(StateValue),
  /// A timer duration.
  Duration(Duration),
}

impl Sample {
  pub fn encoding(&self) -> Encoding {
    match self {
      Sample::Count(_) => Encoding::Count,
      Sample::Number(n) => n.encoding(),
      Sample::State(_) => Encoding::State,
      Sample::Duration(_) => Encoding::Histogram,
    }
  }

  /// Whether a series of this kind accepts the sample.
  pub fn fits(&self, kind: Kind) -> bool {
    matches!(
      (kind, self),
      (Kind::Counter, Sample::Count(_))
        | (Kind::Gauge, Sample::Number(_))
        | (Kind::State, Sample::State(_))
        | (Kind::Timer, Sample::Duration(_))
    )
  }
}

/// One recording of one metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
  pub name: Name,
  pub kind: Kind,
  pub tags: Tags,
  /// Nanoseconds since the Unix epoch.
  pub at_ns: u64,
  pub sample: Sample,
  /// When set, the exact sample is kept at this resolution as well as being aggregated.
  pub resolution: Option<Duration>,
}

/// Where a recorder reads the time.
pub trait Clock: Send + Sync {
  /// Nanoseconds since the Unix epoch.
  fn now_ns(&self) -> u64;
}

/// The system clock. Absent on `wasm*-unknown-unknown` without the
/// `wasm-bindgen` feature, where the host passes its own [`Clock`].
#[cfg(any(not(all(target_family = "wasm", target_os = "unknown")), feature = "wasm-bindgen"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

#[cfg(any(not(all(target_family = "wasm", target_os = "unknown")), feature = "wasm-bindgen"))]
impl Clock for SystemClock {
  fn now_ns(&self) -> u64 {
    crate::time::unix_now_ns()
  }
}

/// Adapts an `xs_foundation` clock.
#[cfg(feature = "xs_foundation")]
pub struct XsClock(pub Arc<dyn xs_foundation::time::Clock>);

#[cfg(feature = "xs_foundation")]
impl Clock for XsClock {
  fn now_ns(&self) -> u64 {
    u64::try_from(self.0.now().as_nano()).unwrap_or(0)
  }
}

/// Takes observations.
pub trait Recorder: Send + Sync {
  /// Nanoseconds since the Unix epoch, from the recorder's clock.
  fn now_ns(&self) -> u64;

  fn record(&self, observation: Observation);

  /// Reports a running total for a counter. The recorder remembers the last
  /// total per series and records the difference as a delta. The first total
  /// only sets the baseline. A total below the last one is a restart and is
  /// recorded whole.
  fn total(&self, name: Name, tags: Tags, total: u64);
}

/// Convenience methods on every recorder, including `dyn Recorder`.
pub trait RecorderExt: Recorder {
  /// Records a sample at the recorder's current time.
  fn observe(&self, name: impl Into<Name>, kind: Kind, tags: Tags, sample: Sample) {
    self.record(Observation {
      name: name.into(),
      kind,
      tags,
      at_ns: self.now_ns(),
      sample,
      resolution: None,
    });
  }

  fn incr(&self, name: impl Into<Name>, tags: Tags) {
    self.observe(name, Kind::Counter, tags, Sample::Count(1));
  }

  fn add(&self, name: impl Into<Name>, tags: Tags, delta: u64) {
    self.observe(name, Kind::Counter, tags, Sample::Count(delta));
  }

  fn gauge(&self, name: impl Into<Name>, tags: Tags, value: impl Into<NumberValue>) {
    self.observe(name, Kind::Gauge, tags, Sample::Number(value.into()));
  }

  fn timer(&self, name: impl Into<Name>, tags: Tags, duration: Duration) {
    self.observe(name, Kind::Timer, tags, Sample::Duration(duration));
  }

  fn state(&self, name: impl Into<Name>, tags: Tags, value: impl Into<StateValue>) {
    self.observe(name, Kind::State, tags, Sample::State(value.into()));
  }
}

impl<R: Recorder + ?Sized> RecorderExt for R {}

/// A recorder that adds tags to everything on its way to another recorder.
/// A tag on the observation with the same name wins.
pub struct Context {
  parent: Arc<dyn Recorder>,
  tags: Tags,
}

impl Context {
  pub fn new(parent: Arc<dyn Recorder>, tags: Tags) -> Self {
    Context { parent, tags }
  }

  pub fn tags(&self) -> &Tags {
    &self.tags
  }

  fn with(&self, mut tags: Tags) -> Tags {
    let mut all = Vec::with_capacity(self.tags.len() + tags.len());
    all.extend(self.tags.iter().cloned());
    all.append(&mut tags);
    all
  }
}

impl Recorder for Context {
  fn now_ns(&self) -> u64 {
    self.parent.now_ns()
  }

  fn record(&self, mut observation: Observation) {
    observation.tags = self.with(observation.tags);
    self.parent.record(observation);
  }

  fn total(&self, name: Name, tags: Tags, total: u64) {
    self.parent.total(name, self.with(tags), total);
  }
}

/// A recorder that drops everything. Its clock always reads zero.
#[derive(Debug, Default, Clone, Copy)]
pub struct Noop;

impl Recorder for Noop {
  fn now_ns(&self) -> u64 {
    0
  }

  fn record(&self, _observation: Observation) {}

  fn total(&self, _name: Name, _tags: Tags, _total: u64) {}
}

static GLOBAL: OnceLock<Arc<dyn Recorder>> = OnceLock::new();
static NOOP: Noop = Noop;

/// Installs the recorder the `metric!` handles use. Succeeds once per process.
pub fn set_global(recorder: Arc<dyn Recorder>) -> Result<(), SetGlobalError> {
  GLOBAL.set(recorder).map_err(SetGlobalError)
}

/// The global recorder. A [`Noop`] when none is installed.
pub fn global() -> &'static dyn Recorder {
  match GLOBAL.get() {
    Some(recorder) => &**recorder,
    None => &NOOP,
  }
}

/// The global recorder as an `Arc`, when one is installed.
pub fn global_arc() -> Option<Arc<dyn Recorder>> {
  GLOBAL.get().cloned()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::tag::Tag;
  use parking_lot::Mutex;

  #[derive(Default)]
  struct Capture {
    seen: Mutex<Vec<Observation>>,
    totals: Mutex<Vec<(Name, Tags, u64)>>,
  }

  impl Recorder for Capture {
    fn now_ns(&self) -> u64 {
      42
    }

    fn record(&self, observation: Observation) {
      self.seen.lock().push(observation);
    }

    fn total(&self, name: Name, tags: Tags, total: u64) {
      self.totals.lock().push((name, tags, total));
    }
  }

  #[test]
  fn sample_fits_its_kind() {
    assert!(Sample::Count(1).fits(Kind::Counter));
    assert!(!Sample::Count(1).fits(Kind::Gauge));
    assert!(Sample::Duration(Duration::ZERO).fits(Kind::Timer));
    assert!(Sample::State(true.into()).fits(Kind::State));
    assert!(!Sample::Number(1u8.into()).fits(Kind::Counter));
  }

  #[test]
  fn ext_methods_build_observations() {
    let capture = Capture::default();
    capture.incr("a", vec![]);
    capture.gauge("b", vec![], 5u32);
    capture.timer("c", vec![], Duration::from_millis(2));
    capture.state("d", vec![], 'x');
    let seen = capture.seen.lock();
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[0].kind, Kind::Counter);
    assert_eq!(seen[0].at_ns, 42);
    assert_eq!(seen[1].sample, Sample::Number(5u32.into()));
    assert_eq!(seen[2].sample, Sample::Duration(Duration::from_millis(2)));
    assert_eq!(seen[3].sample, Sample::State(StateValue::Char('x')));
  }

  #[test]
  fn context_prepends_its_tags() {
    let capture = Arc::new(Capture::default());
    let context = Context::new(capture.clone(), vec![Tag::new("app", "x")]);
    context.incr("a", vec![Tag::new("status", "ok")]);
    context.total("t".into(), vec![], 9);
    let seen = capture.seen.lock();
    assert_eq!(seen[0].tags, vec![Tag::new("app", "x"), Tag::new("status", "ok")]);
    let totals = capture.totals.lock();
    assert_eq!(totals[0].1, vec![Tag::new("app", "x")]);
    assert_eq!(totals[0].2, 9);
  }

  #[test]
  fn noop_records_nothing_and_never_reads_a_clock() {
    Noop.incr("a", vec![]);
    assert_eq!(Noop.now_ns(), 0);
  }
}
