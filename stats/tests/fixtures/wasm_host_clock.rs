use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fibre_stats::{Batch, Clock, Stats, StatsConfig, collect, metric, tags, value::NumberValue};

metric! {
  /// Recorded through the global recorder.
  pub static REQUESTS: Counter = "fixture.requests";
  /// Timed with the host's clock.
  pub static LATENCY: Timer = "fixture.latency";
}

#[link(wasm_import_module = "host")]
unsafe extern "C" {
  fn now_ms() -> f64;
}

struct HostClock;

impl Clock for HostClock {
  fn now_ns(&self) -> u64 {
    (unsafe { now_ms() } * 1_000_000.0) as u64
  }
}

static STATS: OnceLock<Stats> = OnceLock::new();
static BATCHES: AtomicU32 = AtomicU32::new(0);
static SERIES: AtomicU32 = AtomicU32::new(0);
static TIMER_MAX_NS: AtomicU64 = AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn registry_count() -> u32 {
  fibre_stats::registry().count() as u32
}

fn listed(name: &str) -> u32 {
  fibre_stats::registry().any(|m| m.name == name) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn listed_local() -> u32 {
  listed("fixture.requests")
}

#[unsafe(no_mangle)]
pub extern "C" fn listed_recorded() -> u32 {
  listed("fixture.dep.recorded")
}

#[unsafe(no_mangle)]
pub extern "C" fn listed_handle() -> u32 {
  listed("fixture.dep.handle")
}

#[unsafe(no_mangle)]
pub extern "C" fn listed_unreferenced() -> u32 {
  listed("fixture.dep.unreferenced")
}

#[unsafe(no_mangle)]
pub extern "C" fn listed_unnamed() -> u32 {
  listed("fixture.unnamed")
}

#[unsafe(no_mangle)]
pub extern "C" fn call_dependency() -> u32 {
  std::hint::black_box(&dep_called::handle::HANDLE);
  dep_called::recording::work(1) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn record_before_global() {
  REQUESTS.incr(tags!());
}

#[unsafe(no_mangle)]
pub extern "C" fn setup() {
  let stats = Stats::new(StatsConfig::with_clock(Arc::new(HostClock)));
  stats.set_sink(Arc::new(|batch: Batch| {
    BATCHES.fetch_add(1, Ordering::Relaxed);
    SERIES.fetch_add(batch.series.len() as u32, Ordering::Relaxed);
    if let Some(series) = batch.series.iter().find(|s| &*s.name == "fixture.latency") {
      let max = series.points.histograms().unwrap().iter().map(|(_, h)| h.max()).max().unwrap();
      TIMER_MAX_NS.store(max.as_nanos() as u64, Ordering::Relaxed);
    }
  }));
  stats.add_collector(Duration::from_secs(1), collect::gauge("fixture.level", vec![], || NumberValue::count(1)));
  fibre_stats::set_global(Arc::new(stats.clone())).ok().unwrap();
  let _ = STATS.set(stats);
}

#[unsafe(no_mangle)]
pub extern "C" fn record() {
  REQUESTS.incr(tags!());
  LATENCY.time(tags!(), || std::hint::black_box(1));
}

#[unsafe(no_mangle)]
pub extern "C" fn tick() {
  let stats = STATS.get().unwrap();
  stats.collect_due();
  stats.flush();
}

#[unsafe(no_mangle)]
pub extern "C" fn batches() -> u32 {
  BATCHES.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn series() -> u32 {
  SERIES.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn timer_max_ms() -> u32 {
  ((TIMER_MAX_NS.load(Ordering::Relaxed) + 500_000) / 1_000_000) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn refused() -> u32 {
  STATS.get().unwrap().refused() as u32
}
