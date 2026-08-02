use crate::driver::{RunError, RunResult, sent_items};
use crate::spec::{Capacity, Cell, Stage};

use std::time::Duration;

/// Calibration probe size. Small enough that the slowest cell (rendezvous at
/// 64x64) still returns promptly, large enough to time meaningfully.
const PILOT_ITEMS: u64 = 512;
/// Floor on the measured window so per-thread startup skew cannot dominate.
const MIN_ITEMS: u64 = 4_096;
const MAX_ITEMS: u64 = 8_000_000;
/// An unbounded channel lets producers run arbitrarily far ahead of consumers,
/// so the whole run can be resident at once. Keep that bounded.
const MAX_ITEMS_UNBOUNDED: u64 = 1_000_000;
/// A handoff op needs its own channel, created before the timed region, so the
/// item count is also the count of channels resident at once.
const MAX_ITEMS_HANDOFF: u64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct Budget {
  pub target: Duration,
  pub samples: usize,
}

impl Default for Budget {
  fn default() -> Self {
    Budget {
      target: Duration::from_millis(60),
      samples: 7,
    }
  }
}

#[derive(Debug, Clone)]
pub struct Measurement {
  pub items: u64,
  /// Millions of items per second, one entry per sample.
  pub throughputs: Vec<f64>,
}

impl Measurement {
  pub fn from_samples(cell: &Cell, items: u64, samples: &[Duration]) -> Measurement {
    let actual = sent_items(cell, items);
    Measurement {
      items: actual,
      throughputs: samples
        .iter()
        .map(|elapsed| throughput(actual, *elapsed))
        .collect(),
    }
  }

  pub fn median(&self) -> f64 {
    let mut sorted = self.throughputs.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted[sorted.len() / 2]
  }

  pub fn min(&self) -> f64 {
    self.throughputs.iter().cloned().fold(f64::INFINITY, f64::min)
  }

  pub fn max(&self) -> f64 {
    self
      .throughputs
      .iter()
      .cloned()
      .fold(f64::NEG_INFINITY, f64::max)
  }
}

/// Throughput spans four orders of magnitude across the matrix, so precision
/// follows magnitude rather than a fixed width.
pub fn format_throughput(value: f64) -> String {
  if value >= 10.0 {
    format!("{:.1}", value)
  } else if value >= 1.0 {
    format!("{:.2}", value)
  } else {
    format!("{:.3}", value)
  }
}

fn throughput(items: u64, elapsed: Duration) -> f64 {
  let secs = elapsed.as_secs_f64();
  if secs <= 0.0 {
    return 0.0;
  }
  items as f64 / secs / 1e6
}

fn scale_to_budget(budget: &Budget, cell: &Cell, items: u64, elapsed: Duration) -> u64 {
  let ceiling = match (cell.stage, cell.capacity) {
    (Stage::Handoff, _) => MAX_ITEMS_HANDOFF,
    (_, Capacity::Unbounded) => MAX_ITEMS_UNBOUNDED,
    _ => MAX_ITEMS,
  };
  let per_item = elapsed.as_secs_f64() / sent_items(cell, items) as f64;
  if per_item <= 0.0 {
    return ceiling;
  }
  ((budget.target.as_secs_f64() / per_item) as u64).clamp(MIN_ITEMS, ceiling)
}

/// Picks the item count that lands this cell near the time budget, so every
/// cell gets comparable measurement time regardless of how fast it is.
///
/// Calibration runs twice: a tiny probe lands in the right order of magnitude,
/// and the warmup at that size (which also pays the lazy-allocation and
/// thread-park costs) supplies the estimate the samples actually use. A single
/// probe is too easily thrown off by a cold first run.
pub fn calibrate(
  budget: &Budget,
  cell: &Cell,
  run: impl Fn(u64) -> RunResult,
) -> Result<u64, RunError> {
  let probe = run(PILOT_ITEMS)?;
  let warmup_items = scale_to_budget(budget, cell, PILOT_ITEMS, probe);

  let warmup = run(warmup_items)?;
  Ok(scale_to_budget(budget, cell, warmup_items, warmup))
}
