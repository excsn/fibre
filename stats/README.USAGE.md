# Usage Guide: fibre_stats

How to define metrics, record them, aggregate them and hand them to a sink, plus how to add collectors and read what a program can emit.

## Table of Contents

* [Core Concepts](#core-concepts)
* [Quick Start](#quick-start)
* [Defining Metrics](#defining-metrics)
  * [Units](#units)
  * [Tags](#tags)
  * [Descriptions](#descriptions)
* [Recording](#recording)
  * [Counting](#counting)
  * [Reporting a Running Total](#reporting-a-running-total)
  * [Setting a Level](#setting-a-level)
  * [Timing](#timing)
  * [Reporting a State](#reporting-a-state)
  * [Recording by Name](#recording-by-name)
* [Installing a Global Recorder](#installing-a-global-recorder)
* [Adding Tags From a Component](#adding-tags-from-a-component)
* [Aggregating and Flushing](#aggregating-and-flushing)
  * [Reading a Batch](#reading-a-batch)
  * [Pumping by Hand](#pumping-by-hand)
  * [Refusals](#refusals)
* [Running Collectors](#running-collectors)
  * [Process, Host and Tokio](#process-host-and-tokio)
* [Keeping Exact Samples](#keeping-exact-samples)
* [Listing Every Metric](#listing-every-metric)
* [Choosing a Clock](#choosing-a-clock)
* [Running on Wasm](#running-on-wasm)
  * [With wasm-bindgen](#with-wasm-bindgen)
  * [Without wasm-bindgen](#without-wasm-bindgen)
  * [Making Sure the Registry Is Filled](#making-sure-the-registry-is-filled)
* [Why the Recorder Aggregates](#why-the-recorder-aggregates)
* [Error Handling](#error-handling)

## Core Concepts

* **Metric**: the facts about one measurement, being its name, kind, unit, description and allowed tags, written once with `metric!`.
* **Handle**: what `metric!` produces, a `Counter`, `Gauge`, `Timer` or `State` that records through its definition.
* **Kind**: what a metric measures. A counter is a delta, a gauge a level, a timer a duration and a state a boolean, character or enum label.
* **Tag**: a `name = value` pair. The tags on a recording pick the series it lands in.
* **Series**: one metric name with one set of tags.
* **Recorder**: where recordings go. `Stats` aggregates; `Context` adds tags; `Noop` drops.
* **Window**: the span one point covers, one second by default. Every recording in a window becomes one point.
* **Histogram**: what a timer's window holds, with exponential buckets that merge in any order.
* **Batch**: the closed windows of every series since the last flush, handed to a sink.
* **Sink**: the receiver of batches. A function works.
* **Collector**: something that reads a value when asked. `Stats` runs them on a schedule.
* **Registry**: every metric definition the linker kept, filled by constructors the `inventory` crate emits. A definition whose handle the program refers to is always kept.
* **Global recorder**: the one the handles use when no recorder is passed.
* **Encoding**: how a series' values are stored, fixed by the first recording.

## Quick Start

```rust
use std::sync::Arc;
use std::time::Duration;

use fibre_stats::{metric, tags, Batch, Stats, StatsConfig};

metric! {
  /// Requests answered.
  #[tags("status", "route")]
  pub static REQUESTS: Counter = "http.requests";

  /// Time to answer one request.
  #[tags("route")]
  pub static LATENCY: Timer = "http.latency";

  /// Connections open right now.
  pub static CONNECTIONS: Gauge = "http.connections";
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let stats = Stats::new(StatsConfig {
    common_tags: tags!("app" => "shop"),
    ..StatsConfig::default()
  });
  stats.start(Arc::new(|batch: Batch| {
    for series in &batch.series {
      println!("{} {:?} {} points", series.name, series.tags, series.points.len());
    }
  }));
  fibre_stats::set_global(Arc::new(stats.clone()))?;

  REQUESTS.incr(tags!("status" => "ok", "route" => "/"));
  LATENCY.time(tags!("route" => "/"), || std::thread::sleep(Duration::from_millis(2)));
  CONNECTIONS.set(12u32, tags!());

  stats.stop();
  Ok(())
}
```

## Defining Metrics

One `metric!` block holds any number of definitions. Each is a `static` whose type is the handle and whose value is the metric name.

```rust
use fibre_stats::metric;

metric! {
  /// Jobs run to completion or failure.
  #[tags("job", "outcome")]
  pub static JOB_RUNS: Counter = "job.runs";

  /// How long a job ran.
  #[tags("job")]
  pub static JOB_DURATION: Timer = "job.duration";

  /// Jobs waiting for a worker.
  pub static JOB_QUEUE_DEPTH: Gauge = "job.queue_depth";

  /// Where the scheduler is in its life.
  pub static SCHEDULER_STATUS: State = "job.scheduler.status";
}
```

A definition registers itself when the program loads, so nothing has to be called. Put `metric!` at module level. The linker can drop a definition in a library when the program never refers to its handle, so a tool that lists a library's metrics should refer to each handle it needs listed, for example with `std::hint::black_box(&REQUESTS)`. A metric the program records already refers to its handle.

### Units

A counter is `count` and a timer is `seconds` unless the definition says otherwise. A gauge says what it holds.

```rust
use fibre_stats::metric;

metric! {
  #[unit(bytes)]
  pub static HEAP_BYTES: Gauge = "alloc.heap_bytes";

  #[unit(percent)]
  pub static CACHE_HIT_PCT: Gauge = "cache.hit_pct";

  #[unit("rpm")]
  pub static FAN_SPEED: Gauge = "fan.speed";
}
```

The named units are `none`, `count`, `seconds`, `bytes`, `percent`, `ratio`, `celsius` and `hertz`. A string is anything else.

### Tags

`#[tags(...)]` lists the tag names a recording may carry. A recording with any other tag panics in a debug build and records in a release build, so the mistake is found while developing. Tags added by a `Context` or `common_tags` are not checked, since those come from the deployment rather than the call site.

```rust
use fibre_stats::{metric, tags};

metric! {
  #[tags("status")]
  pub static REQUESTS: Counter = "http.requests";
}

REQUESTS.incr(tags!("status" => "ok"));
```

### Descriptions

The doc comment is the description. It travels with the definition into the registry, so whatever reads the registry can show it.

```rust
use fibre_stats::metric;

metric! {
  /// Bytes read from disk since the previous read.
  /// Summed per second.
  #[unit(bytes)]
  pub static DISK_READ_BYTES: Counter = "process.disk.read_bytes";
}

assert_eq!(DISK_READ_BYTES.metric.description_text(), "Bytes read from disk since the previous read.\nSummed per second.");
```

## Recording

Every handle has a method that records on the global recorder and a `_to` twin that takes a recorder. Use the twin in a library that is handed its recorder and in tests.

### Counting

```rust
use fibre_stats::{metric, tags, Stats, StatsConfig};

metric! {
  #[tags("status")]
  pub static REQUESTS: Counter = "http.requests";
}

let stats = Stats::new(StatsConfig::default());
REQUESTS.add_to(&stats, 1, tags!("status" => "ok"));
REQUESTS.incr(tags!("status" => "ok"));
```

Deltas in one window are summed into one point.

### Reporting a Running Total

Some things keep their own total, such as bytes sent on a socket. Report the total and the recorder turns it into deltas: the first total only sets the baseline, each later total records the difference and a total below the last one is a restart and is recorded whole.

```rust
use fibre_stats::{metric, tags, Stats, StatsConfig};

metric! {
  #[unit(bytes)]
  pub static SENT_BYTES: Counter = "socket.sent_bytes";
}

let stats = Stats::new(StatsConfig::default());
SENT_BYTES.total_to(&stats, 1_000, tags!());
SENT_BYTES.total_to(&stats, 1_500, tags!());
let batch = stats.drain(true);
assert_eq!(batch.series[0].points.counts().unwrap()[0].1, 500);
```

### Setting a Level

A gauge takes any integer, a `Percent`, a `Normalized`, a `Ratio32` or a `NumberValue`. The last level set in a window is the point.

```rust
use fibre_stats::{metric, tags, NumberValue, Stats, StatsConfig};

metric! {
  pub static QUEUE_DEPTH: Gauge = "job.queue_depth";
  #[unit(percent)]
  pub static CPU_PCT: Gauge = "process.cpu_pct";
}

let stats = Stats::new(StatsConfig::default());
QUEUE_DEPTH.set_to(&stats, 7usize, tags!());
CPU_PCT.set_to(&stats, NumberValue::percent_saturating(42.5), tags!());
```

A series keeps the encoding of its first value. A gauge first set with an integer refuses a later `Percent`; see [Refusals](#refusals).

### Timing

Three shapes, for a duration you already hold, a closure and a scope.

```rust
use std::time::Duration;
use fibre_stats::{metric, tags, Stats, StatsConfig};

metric! {
  #[tags("route", "outcome")]
  pub static LATENCY: Timer = "http.latency";
}

let stats = Stats::new(StatsConfig::default());

LATENCY.record_to(&stats, Duration::from_millis(3), tags!("route" => "/", "outcome" => "ok"));

let body = LATENCY.time_to(&stats, tags!("route" => "/", "outcome" => "ok"), || "hello");

{
  let mut guard = LATENCY.guard_to(&stats, tags!("route" => "/", "outcome" => "ok"));
  let failed = false;
  if failed {
    guard.set_tags(tags!("route" => "/", "outcome" => "error"));
  }
}
```

The guard records when it drops. `guard.stop()` records now and `guard.discard()` records nothing, for a request that was cancelled rather than answered.

Each window holds a `Histogram` of the durations in it. Percentiles are read from the histogram, in the sink or wherever the batch ends up.

### Reporting a State

A state is a boolean, a character or an enum label. `state_enum!` writes a Rust enum whose variants carry their labels. The label is what a store keys the variant by, so a variant can be renamed without changing the series.

```rust
use fibre_stats::{metric, state_enum, tags, Stats, StatsConfig};

state_enum! {
  pub enum SchedulerStatus {
    Starting = "starting",
    Running = "running",
    Draining = "draining",
  }
}

metric! {
  pub static SCHEDULER_STATUS: State = "job.scheduler.status";
  pub static LEADER: State = "cluster.leader";
}

let stats = Stats::new(StatsConfig::default());
SCHEDULER_STATUS.set_to(&stats, SchedulerStatus::Running, tags!());
LEADER.set_to(&stats, true, tags!());
```

### Recording by Name

For a name only known at runtime, `RecorderExt` adds `incr`, `add`, `gauge`, `timer` and `state` to every recorder, including `dyn Recorder`. Nothing checks the tags and nothing is added to the registry.

```rust
use std::time::Duration;
use fibre_stats::{tags, RecorderExt, Stats, StatsConfig};

let stats = Stats::new(StatsConfig::default());
let name = format!("plugin.{}.calls", "auth");
stats.incr(name.clone(), tags!());
stats.gauge(name.clone(), tags!(), 3u8);
stats.timer("plugin.auth.duration", tags!(), Duration::from_millis(1));
```

## Installing a Global Recorder

The handles' plain methods record on the global recorder. Until one is installed they record on a `Noop`, so a library can record before its host has set anything up.

```rust
use std::sync::Arc;
use fibre_stats::{Stats, StatsConfig};

let stats = Stats::new(StatsConfig::default());
if let Err(e) = fibre_stats::set_global(Arc::new(stats.clone())) {
  eprintln!("{e}");
}
```

`set_global` succeeds once per process. Anything that needs the global as an `Arc`, such as a `Context`, gets it from `global_arc()`.

## Adding Tags From a Component

A `Context` adds the same tags to everything recorded through it. A tag on the recording with the same name wins.

```rust
use std::sync::Arc;
use fibre_stats::{tags, Context, Recorder, RecorderExt, Stats, StatsConfig};

let stats = Stats::new(StatsConfig::default());
let pool: Arc<dyn Recorder> = stats.context(tags!("pool" => "rpc"));
pool.gauge("pool.active", tags!(), 4u32);

let other = Context::new(Arc::new(stats.clone()), tags!("pool" => "stream"));
other.gauge("pool.active", tags!(), 1u32);
```

## Aggregating and Flushing

`Stats` is the recorder that aggregates. `StatsConfig` sets the common tags, the window, the flush interval and the clock.

```rust
use std::sync::Arc;
use std::time::Duration;
use fibre_stats::{tags, Batch, Stats, StatsConfig, VecSink};

let stats = Stats::new(StatsConfig {
  common_tags: tags!("env" => "prod", "app" => "shop"),
  resolution: Duration::from_secs(1),
  flush_interval: Duration::from_secs(10),
  ..StatsConfig::default()
});

let sink = Arc::new(VecSink::new());
stats.start(sink.clone());
```

`start` sets the sink and starts the flush thread, which hands every closed window to the sink each interval. `stop` stops the threads, waits for them and flushes everything left, open windows included. Dropping the last `Stats` clone also flushes what is left, without waiting for the threads.

A sink is anything with `push(&self, Batch)`. A closure works and so does any `Arc<T: Sink>`.

```rust
use std::sync::Arc;
use fibre_stats::{Batch, Stats, StatsConfig};

let stats = Stats::new(StatsConfig::default());
stats.start(Arc::new(|batch: Batch| {
  println!("{} series, {} dropped", batch.series.len(), batch.dropped.len());
}));
```

### Reading a Batch

A batch carries the common tags once, then one `SeriesBatch` per series with its kind, encoding, own tags and points in time order. Timestamps are nanoseconds since the Unix epoch and name the start of the window.

```rust
use std::time::Duration;
use fibre_stats::{metric, tags, Points, Stats, StatsConfig};

metric! {
  #[tags("route")]
  pub static LATENCY: Timer = "http.latency";
  #[tags("route")]
  pub static REQUESTS: Counter = "http.requests";
}

let stats = Stats::new(StatsConfig::default());
LATENCY.record_to(&stats, Duration::from_millis(5), tags!("route" => "/"));
REQUESTS.add_to(&stats, 1, tags!("route" => "/"));

let batch = stats.drain(true);
for series in &batch.series {
  match &series.points {
    Points::Counts(points) => for (at_ns, count) in points {
      println!("{} {at_ns} {count}", series.name);
    },
    Points::Histograms(points) => for (at_ns, histogram) in points {
      println!("{} {at_ns} p99 {:?} max {:?}", series.name, histogram.quantile(0.99), histogram.max());
    },
    Points::Numbers(_) | Points::States(_) | Points::Samples(_) => {}
  }
}
```

`Batch::series(name, &tags)` finds one series by name and exact tags, in any tag order.

### Pumping by Hand

Without `start` nothing runs in the background. `drain(false)` takes the closed windows, `drain(true)` takes the open one too and `flush()` hands the closed windows to a sink set with `set_sink`. Tests use `drain(true)` with a fixed clock.

```rust
use std::sync::Arc;
use fibre_stats::{tags, Clock, RecorderExt, Stats, StatsConfig};

struct Fixed(u64);
impl Clock for Fixed {
  fn now_ns(&self) -> u64 { self.0 }
}

let stats = Stats::new(StatsConfig { clock: Arc::new(Fixed(5_000_000_000)), ..StatsConfig::default() });
stats.incr("a", tags!());
let batch = stats.drain(true);
assert_eq!(batch.series[0].points.counts().unwrap()[0], (5_000_000_000, 1));
```

### Refusals

A recording is refused, counted and otherwise dropped when its sample does not fit the kind, when the series was first recorded with another kind or when the series was first recorded with another encoding. `refused()` is the count and `last_refusal()` says what happened last, with the series and the reason.

```rust
use fibre_stats::{tags, NumberValue, RecorderExt, RefusalReason, Stats, StatsConfig};

let stats = Stats::new(StatsConfig::default());
stats.gauge("depth", tags!(), 1i64);
stats.gauge("depth", tags!(), NumberValue::count(2));
assert_eq!(stats.refused(), 1);
assert!(matches!(stats.last_refusal().unwrap().reason, RefusalReason::EncodingChanged { .. }));
```

## Running Collectors

A collector reads a value when asked. `add_collector` runs one every interval on the collect thread, first after one interval. `collect_now` runs every collector on the calling thread, for tests and for a last read before shutdown. A collector must not add or remove collectors.

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use fibre_stats::{collect, metric, tags, NumberValue, Recorder, RecorderExt, Stats, StatsConfig};

metric! {
  #[tags("pool")]
  pub static POOL_ACTIVE: Gauge = "pool.active";
}

let active = Arc::new(AtomicUsize::new(0));
let stats = Stats::new(StatsConfig::default());

let read = active.clone();
stats.add_collector(Duration::from_secs(10), POOL_ACTIVE.poll(tags!("pool" => "rpc"), move || {
  NumberValue::from(read.load(Ordering::Relaxed))
}));

stats.add_collector(Duration::from_secs(10), collect::gauge("pool.queued", tags!(), || NumberValue::count(0)));

stats.add_collector(Duration::from_secs(10), |recorder: &dyn Recorder| {
  recorder.gauge("custom.level", tags!(), 1u8);
});

stats.collect_now();
```

`add_collector_with_tags` adds tags to everything one collector records, which is how a collector written without tags is run once per instance.

### Process, Host and Tokio

Each feature adds a collector whose metrics are defined with `metric!`, so a program that turns the feature on reports the same series as any other.

```toml
[dependencies]
fibre_stats = { version = "0", features = ["collect-process", "collect-host", "collect-tokio"] }
```

```rust,ignore
use std::time::Duration;
use fibre_stats::{collect, tags};

stats.add_collector(Duration::from_secs(10), collect::host()?);
stats.add_collector_with_tags(tags!("instance" => "shop_1"), Duration::from_secs(10), collect::process(1234)?);
stats.add_collector(Duration::from_secs(10), collect::tokio(tokio::runtime::Handle::current()));
```

`collect::process` records `process.alive`, `process.cpu_pct` as a share of every core, `process.memory_bytes`, `process.virtual_memory_bytes`, `process.threads` on Linux, `process.open_fds` where the platform says, `process.disk.read_bytes`, `process.disk.written_bytes` and `process.run_time_secs`. `collect::host` records memory, swap, CPU, load, uptime, network counters per interface, disk space per mount and temperatures per component under `host.*`. `collect::tokio` records workers, alive tasks, the global queue depth and per worker busy percentage and parks under `tokio.*`, from what stable tokio exposes. CPU percentages are zero on the first read, since they need two reads to measure.

## Keeping Exact Samples

Aggregation loses the individual values. A metric that needs them declares a resolution and, with the `high_res` feature on, every sample is kept beside the aggregate until the next flush. The buffer is bounded by `high_res_buffer` per series; when it fills, the oldest sample is dropped and the batch reports how many were dropped and when, so a reader can mark the gap.

```rust
use fibre_stats::metric;

metric! {
  /// Time for one simulation tick.
  #[high_res("1ms")]
  pub static TICK: Timer = "sim.tick";
}
```

The samples arrive as `Points::Samples` on a series whose `resolution` is the declared one, next to the per-second series with the histograms. Without the feature the attribute is kept in the definition and nothing else changes.

## Listing Every Metric

`registry()` iterates every definition the linker kept, from every crate. `find(name)` gets one.

```rust
use fibre_stats::{metric, registry};

metric! {
  /// Requests answered.
  #[tags("status")]
  pub static REQUESTS: Counter = "http.requests";
}

for metric in registry() {
  println!("{} {} {} {:?}", metric.name, metric.kind, metric.unit.as_str(), metric.tags);
}
let requests = fibre_stats::find("http.requests").unwrap();
assert_eq!(requests.description_text(), "Requests answered.");
```

This is what a program sends to whatever stores its metrics, so names, kinds, units and descriptions never have to be written a second time.

## Choosing a Clock

`StatsConfig::clock` is where timestamps come from. The default reads the system clock. A test passes a fixed one. With the `xs_foundation` feature, `XsClock` wraps an `xs_foundation::time::Clock`.

```rust,ignore
use std::sync::Arc;
use fibre_stats::{Stats, StatsConfig, XsClock};

let clock: Arc<dyn xs_foundation::time::Clock> = Arc::new(xs_foundation::time::UtcClock::new());
let stats = Stats::new(StatsConfig { clock: Arc::new(XsClock(clock)), ..StatsConfig::default() });
```

## Running on Wasm

Wasm targets have no threads, so `start` does not exist there. `add_collector` registers a collector and spawns nothing. The host drives the same work from its own timer: `collect_due` runs what is due and `flush` hands closed windows to the sink set with `set_sink`. The `collect-process`, `collect-host` and `collect-tokio` collectors do not exist on wasm.

`std::time` panics on `wasm*-unknown-unknown`, so the clock depends on how the module is built. Other wasm targets such as `wasm32-wasip1` use `std::time` and need neither of the two setups below.

### With wasm-bindgen

The feature reads `Date.now()` and `Performance.now()` through `web-time`, so `SystemClock` and `StatsConfig::default()` work as they do natively.

```toml
[dependencies]
fibre_stats = { version = "0", features = ["wasm-bindgen"] }
```

```rust,ignore
use std::sync::Arc;
use fibre_stats::{Batch, Stats, StatsConfig};

let stats = Stats::new(StatsConfig::default());
stats.set_sink(Arc::new(|batch: Batch| send_beacon(batch)));
fibre_stats::set_global(Arc::new(stats.clone()))?;

// From a setInterval callback once a second:
stats.collect_due();
stats.flush();
```

### Without wasm-bindgen

For a module whose JavaScript is written by hand, the host passes a clock. There is no `SystemClock` and no `StatsConfig::default()`, so forgetting it is a compile error. Timers measure with the same clock.

```rust,ignore
use std::sync::Arc;
use fibre_stats::{Clock, Stats, StatsConfig};

unsafe extern "C" {
  fn date_now_ms() -> f64;
}

struct HostClock;

impl Clock for HostClock {
  fn now_ns(&self) -> u64 {
    (unsafe { date_now_ms() } * 1_000_000.0) as u64
  }
}

let stats = Stats::new(StatsConfig::with_clock(Arc::new(HostClock)));
```

### Making Sure the Registry Is Filled

Definitions register through constructors, which on wasm run from `__wasm_call_ctors`. In a `cdylib` the linker inserts a call to it at the start of each exported function, so the registry is filled by the time any export runs. This holds with and without `wasm-bindgen`.

The linker keeps less on wasm than on native targets. A library's definition can be missing even when the program calls other code in that library. `use library as _;` does not bring it back. Refer to the handle to keep it.

## Why the Recorder Aggregates

A counter's deltas within a second add up to the same number whatever order they arrive in. Two histograms add bucket by bucket. So summing in the process loses nothing that a one second point would have. What it saves is the wire: one point per series per second instead of one per call. A gauge keeps only its last level in a window because a level is a reading, not an event. Anything that needs the individual values says so with `#[high_res]` and pays for it only on that series.

Definitions register themselves rather than waiting for a call because a library cannot know whether its host remembered to call anything.

## Error Handling

Recording never fails. A recording the book will not take is counted as a refusal and reported through `refused()` and `last_refusal()`. Two operations return errors.

```rust
use std::sync::Arc;
use fibre_stats::{CollectError, SetGlobalError, Stats, StatsConfig};

let stats = Stats::new(StatsConfig::default());
match fibre_stats::set_global(Arc::new(stats.clone())) {
  Ok(()) => {}
  Err(SetGlobalError(not_installed)) => {
    let _ = not_installed;
  }
}

fn describe(e: CollectError) -> String {
  match e {
    CollectError::Unavailable(why) => why,
  }
}
```

`SetGlobalError` holds the recorder that was not installed, so a caller can keep using it directly. `CollectError::Unavailable` comes from `collect::process` and `collect::host` when the platform or `sysinfo` cannot provide what they read.
