//! Collectors read a value on a schedule and record it.
//!
//! A [`Collector`] is anything with a `collect` method, including a closure.
//! `Stats::add_collector` runs one at an interval on the collect thread. The
//! optional features add collectors for a process, the host and a tokio
//! runtime, each defining its metrics with `metric!`. Those three do not exist
//! on wasm targets.

use crate::metric::Kind;
use crate::recorder::{Observation, Recorder, Sample};
use crate::tag::{Name, Tags};
use crate::value::{NumberValue, StateValue};

#[cfg(all(feature = "collect-host", not(target_family = "wasm")))]
pub mod host;
#[cfg(all(feature = "collect-process", not(target_family = "wasm")))]
pub mod process;
#[cfg(all(feature = "collect-tokio", not(target_family = "wasm")))]
pub mod tokio;

#[cfg(all(feature = "collect-host", not(target_family = "wasm")))]
pub use host::{HostCollector, host};
#[cfg(all(feature = "collect-process", not(target_family = "wasm")))]
pub use process::{ProcessCollector, process};
#[cfg(all(feature = "collect-tokio", not(target_family = "wasm")))]
pub use tokio::{TokioCollector, tokio};

/// Something that records when asked.
pub trait Collector: Send + 'static {
  fn collect(&mut self, recorder: &dyn Recorder);
}

impl<F: FnMut(&dyn Recorder) + Send + 'static> Collector for F {
  fn collect(&mut self, recorder: &dyn Recorder) {
    self(recorder)
  }
}

/// A collector that records a gauge from a function.
pub struct PollGauge<F> {
  name: Name,
  tags: Tags,
  read: F,
}

impl<F: Fn() -> NumberValue + Send + 'static> PollGauge<F> {
  pub fn new(name: Name, tags: Tags, read: F) -> Self {
    PollGauge { name, tags, read }
  }
}

impl<F: Fn() -> NumberValue + Send + 'static> Collector for PollGauge<F> {
  fn collect(&mut self, recorder: &dyn Recorder) {
    recorder.record(Observation {
      name: self.name.clone(),
      kind: Kind::Gauge,
      tags: self.tags.clone(),
      at_ns: recorder.now_ns(),
      sample: Sample::Number((self.read)()),
      resolution: None,
    });
  }
}

/// A collector that records a state from a function.
pub struct PollState<F> {
  name: Name,
  tags: Tags,
  read: F,
}

impl<F: Fn() -> StateValue + Send + 'static> PollState<F> {
  pub fn new(name: Name, tags: Tags, read: F) -> Self {
    PollState { name, tags, read }
  }
}

impl<F: Fn() -> StateValue + Send + 'static> Collector for PollState<F> {
  fn collect(&mut self, recorder: &dyn Recorder) {
    recorder.record(Observation {
      name: self.name.clone(),
      kind: Kind::State,
      tags: self.tags.clone(),
      at_ns: recorder.now_ns(),
      sample: Sample::State((self.read)()),
      resolution: None,
    });
  }
}

/// A collector that reports a running total from a function. See [`Recorder::total`].
pub struct PollTotal<F> {
  name: Name,
  tags: Tags,
  read: F,
}

impl<F: Fn() -> u64 + Send + 'static> PollTotal<F> {
  pub fn new(name: Name, tags: Tags, read: F) -> Self {
    PollTotal { name, tags, read }
  }
}

impl<F: Fn() -> u64 + Send + 'static> Collector for PollTotal<F> {
  fn collect(&mut self, recorder: &dyn Recorder) {
    recorder.total(self.name.clone(), self.tags.clone(), (self.read)());
  }
}

/// A gauge collector by name, for a metric without a `metric!` definition.
pub fn gauge<F>(name: impl Into<Name>, tags: Tags, read: F) -> PollGauge<F>
where
  F: Fn() -> NumberValue + Send + 'static,
{
  PollGauge::new(name.into(), tags, read)
}

/// A state collector by name.
pub fn state<F>(name: impl Into<Name>, tags: Tags, read: F) -> PollState<F>
where
  F: Fn() -> StateValue + Send + 'static,
{
  PollState::new(name.into(), tags, read)
}

/// A running total collector by name.
pub fn total<F>(name: impl Into<Name>, tags: Tags, read: F) -> PollTotal<F>
where
  F: Fn() -> u64 + Send + 'static,
{
  PollTotal::new(name.into(), tags, read)
}
