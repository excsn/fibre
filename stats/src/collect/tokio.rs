//! Runtime statistics for a tokio runtime, from `Handle::metrics`. Only what
//! stable tokio exposes; nothing here needs `tokio_unstable`.

use std::time::{Duration, Instant};

use tokio::runtime::Handle;

use crate::collect::Collector;
use crate::recorder::Recorder;
use crate::tag::Tag;
use crate::value::NumberValue;

crate::metric! {
  /// Worker threads in the runtime.
  pub static WORKERS: Gauge = "tokio.workers";
  /// Tasks alive at the time of the read.
  pub static TASKS_ALIVE: Gauge = "tokio.tasks.alive";
  /// Tasks waiting in the global queue, not yet taken by a worker.
  pub static QUEUE_GLOBAL: Gauge = "tokio.queue.global";
  /// Share of the time since the previous read a worker spent running tasks.
  #[unit(percent)]
  #[tags("worker")]
  pub static WORKER_BUSY_PCT: Gauge = "tokio.worker.busy_pct";
  /// Times a worker parked since the previous read.
  #[tags("worker")]
  pub static WORKER_PARKS: Counter = "tokio.worker.parks";
}

/// Records the `tokio.*` metrics for one runtime each time it runs.
pub struct TokioCollector {
  handle: Handle,
  last_at: Instant,
  last_busy: Vec<Duration>,
  last_parks: Vec<u64>,
}

/// A collector for the runtime behind `handle`.
pub fn tokio(handle: Handle) -> TokioCollector {
  TokioCollector {
    handle,
    last_at: Instant::now(),
    last_busy: Vec::new(),
    last_parks: Vec::new(),
  }
}

impl Collector for TokioCollector {
  fn collect(&mut self, recorder: &dyn Recorder) {
    let metrics = self.handle.metrics();
    let workers = metrics.num_workers();
    WORKERS.set_to(recorder, workers, Vec::new());
    TASKS_ALIVE.set_to(recorder, metrics.num_alive_tasks(), Vec::new());
    QUEUE_GLOBAL.set_to(recorder, metrics.global_queue_depth(), Vec::new());
    self.collect_workers(recorder, &metrics, workers);
  }
}

impl TokioCollector {
  #[cfg(target_has_atomic = "64")]
  fn collect_workers(&mut self, recorder: &dyn Recorder, metrics: &tokio::runtime::RuntimeMetrics, workers: usize) {
    let now = Instant::now();
    let wall = now.saturating_duration_since(self.last_at);
    let had_baseline = self.last_busy.len() == workers && self.last_parks.len() == workers;
    let mut busy_now = Vec::with_capacity(workers);
    let mut parks_now = Vec::with_capacity(workers);
    for worker in 0..workers {
      let busy = metrics.worker_total_busy_duration(worker);
      let parks = metrics.worker_park_count(worker);
      if had_baseline {
        let tags = || vec![Tag::new("worker", worker.to_string())];
        if wall > Duration::ZERO {
          let pct = busy.saturating_sub(self.last_busy[worker]).as_secs_f32() / wall.as_secs_f32() * 100.0;
          WORKER_BUSY_PCT.set_to(recorder, NumberValue::percent_saturating(pct), tags());
        }
        WORKER_PARKS.add_to(recorder, parks.saturating_sub(self.last_parks[worker]), tags());
      }
      busy_now.push(busy);
      parks_now.push(parks);
    }
    self.last_busy = busy_now;
    self.last_parks = parks_now;
    self.last_at = now;
  }

  #[cfg(not(target_has_atomic = "64"))]
  fn collect_workers(&mut self, _recorder: &dyn Recorder, _metrics: &tokio::runtime::RuntimeMetrics, _workers: usize) {}
}
