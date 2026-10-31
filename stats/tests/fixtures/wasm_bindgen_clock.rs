use std::sync::Arc;
use std::time::Duration;

use fibre_stats::{Batch, Recorder, Stats, StatsConfig, collect, metric, tags, value::NumberValue};
use wasm_bindgen::prelude::*;

metric! {
  /// Recorded through the global recorder.
  pub static REQUESTS: Counter = "fixture.requests";
  /// Timed with the browser's clock.
  pub static LATENCY: Timer = "fixture.latency";
}

#[wasm_bindgen]
pub fn run() -> String {
  let count = fibre_stats::registry().count();
  let stats = Stats::new(StatsConfig::default());
  stats.set_sink(Arc::new(|_: Batch| {}));
  stats.add_collector(Duration::ZERO, collect::gauge("fixture.level", vec![], || NumberValue::count(1)));
  fibre_stats::set_global(Arc::new(stats.clone())).ok().unwrap();
  REQUESTS.incr(tags!());
  LATENCY.time(tags!(), || std::hint::black_box(1));
  stats.collect_due();
  let now_ms = stats.now_ns() / 1_000_000;
  let batch = stats.drain(true);
  let timer = batch.series.iter().find(|s| &*s.name == "fixture.latency").map(|s| s.points.histograms().unwrap().len()).unwrap_or(0);
  format!(
    "registry={count}\nnow_ms={now_ms}\nseries={}\ntimer_points={timer}\nrefused={}\n",
    batch.series.len(),
    stats.refused()
  )
}
