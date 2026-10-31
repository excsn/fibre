//! What a [`Stats`](crate::Stats) hands on when it flushes.

use std::time::Duration;

use parking_lot::Mutex;

use crate::histogram::Histogram;
use crate::metric::Kind;
use crate::recorder::Sample;
use crate::tag::{Name, Tags};
use crate::value::{Encoding, NumberValue, StateValue};

/// Receives batches. Called on the flush thread, so a slow sink delays the
/// next flush and nothing else.
pub trait Sink: Send + Sync {
  fn push(&self, batch: Batch);
}

impl<F: Fn(Batch) + Send + Sync> Sink for F {
  fn push(&self, batch: Batch) {
    self(batch)
  }
}

/// A sink that keeps every batch, for tests and for pumping by hand.
#[derive(Default)]
pub struct VecSink {
  batches: Mutex<Vec<Batch>>,
}

impl VecSink {
  pub fn new() -> Self {
    VecSink::default()
  }

  /// Removes and returns every batch received so far.
  pub fn take(&self) -> Vec<Batch> {
    std::mem::take(&mut *self.batches.lock())
  }

  pub fn len(&self) -> usize {
    self.batches.lock().len()
  }

  pub fn is_empty(&self) -> bool {
    self.batches.lock().is_empty()
  }
}

impl Sink for VecSink {
  fn push(&self, batch: Batch) {
    self.batches.lock().push(batch);
  }
}

/// Everything closed since the last flush.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Batch {
  /// Tags every series in the batch carries. They are not repeated on the series.
  pub common_tags: Tags,
  pub series: Vec<SeriesBatch>,
  /// High resolution samples that did not fit their buffer.
  pub dropped: Vec<Dropped>,
}

impl Batch {
  pub fn is_empty(&self) -> bool {
    self.series.is_empty() && self.dropped.is_empty()
  }

  /// The series with this name and exactly these tags, in any order.
  pub fn series(&self, name: &str, tags: &Tags) -> Option<&SeriesBatch> {
    self.series.iter().find(|s| {
      s.name == name && s.tags.len() == tags.len() && tags.iter().all(|t| s.tags.contains(t))
    })
  }
}

/// One series' points since the last flush, in time order.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesBatch {
  pub name: Name,
  pub kind: Kind,
  pub encoding: Encoding,
  /// The series' own tags, without the batch's common tags.
  pub tags: Tags,
  /// The window each point covers. For `Points::Samples` it is the sample resolution.
  pub resolution: Duration,
  pub points: Points,
}

/// The points of one series. Timestamps are nanoseconds since the Unix epoch
/// and name the start of the window. For `Samples` they are the sample's own time.
#[derive(Debug, Clone, PartialEq)]
pub enum Points {
  /// Counter deltas summed per window.
  Counts(Vec<(u64, u64)>),
  /// The last gauge level per window.
  Numbers(Vec<(u64, NumberValue)>),
  /// The last state per window.
  States(Vec<(u64, StateValue)>),
  /// Timer durations per window.
  Histograms(Vec<(u64, Histogram)>),
  /// Exact samples from a high resolution series.
  Samples(Vec<(u64, Sample)>),
}

impl Points {
  pub fn len(&self) -> usize {
    match self {
      Points::Counts(v) => v.len(),
      Points::Numbers(v) => v.len(),
      Points::States(v) => v.len(),
      Points::Histograms(v) => v.len(),
      Points::Samples(v) => v.len(),
    }
  }

  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  pub fn counts(&self) -> Option<&[(u64, u64)]> {
    match self {
      Points::Counts(v) => Some(v),
      _ => None,
    }
  }

  pub fn numbers(&self) -> Option<&[(u64, NumberValue)]> {
    match self {
      Points::Numbers(v) => Some(v),
      _ => None,
    }
  }

  pub fn states(&self) -> Option<&[(u64, StateValue)]> {
    match self {
      Points::States(v) => Some(v),
      _ => None,
    }
  }

  pub fn histograms(&self) -> Option<&[(u64, Histogram)]> {
    match self {
      Points::Histograms(v) => Some(v),
      _ => None,
    }
  }

  pub fn samples(&self) -> Option<&[(u64, Sample)]> {
    match self {
      Points::Samples(v) => Some(v),
      _ => None,
    }
  }
}

/// High resolution samples of one series that were dropped because the buffer
/// was full. `from_ns` and `to_ns` are the times of the first and last dropped
/// sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
  pub name: Name,
  pub tags: Tags,
  pub count: u64,
  pub from_ns: u64,
  pub to_ns: u64,
}
