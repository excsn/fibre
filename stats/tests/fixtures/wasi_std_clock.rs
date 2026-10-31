use std::sync::Arc;
use std::time::Duration;

use fibre_stats::{Recorder, Stats, StatsConfig, collect, metric, tags, value::NumberValue};

metric! {
  /// Recorded through the global recorder.
  pub static REQUESTS: Counter = "fixture.requests";
  /// Timed with std::time.
  pub static LATENCY: Timer = "fixture.latency";
}

fn main() {
  let stats = Stats::new(StatsConfig::default());
  stats.add_collector(Duration::ZERO, collect::gauge("fixture.level", vec![], || NumberValue::count(1)));
  fibre_stats::set_global(Arc::new(stats.clone())).ok().unwrap();
  REQUESTS.incr(tags!());
  LATENCY.time(tags!(), || std::hint::black_box(1));
  stats.collect_due();
  println!("registry={}", fibre_stats::registry().count());
  println!("now_ms={}", stats.now_ns() / 1_000_000);
  println!("series={}", stats.drain(true).series.len());
  println!("refused={}", stats.refused());
}
