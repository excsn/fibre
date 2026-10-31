# API Reference: fibre_stats

Metrics defined once with `metric!`, aggregated per window in process and handed to a sink in batches.

## Contents

* [1. Defining Metrics](#1-defining-metrics)
  * [The `metric!` macro](#the-metric-macro)
  * [`state_enum!`](#state_enum)
  * [`tags!`](#tags)
  * [`Metric`](#metric)
  * [`MetricAttr`](#metricattr)
  * [`Kind`](#kind)
  * [`Unit`](#unit)
  * [`registry` and `find`](#registry-and-find)
  * [`parse_duration`](#parse_duration)
* [2. Handles](#2-handles)
  * [`Counter`](#counter)
  * [`Gauge`](#gauge)
  * [`Timer`](#timer)
  * [`TimerGuard`](#timerguard)
  * [`State`](#state)
* [3. Values](#3-values)
  * [`IntValue`](#intvalue)
  * [`NumberValue`](#numbervalue)
  * [`StateValue`](#statevalue)
  * [`StatValue`](#statvalue)
  * [`EnumState`](#enumstate)
  * [`Encoding`](#encoding)
  * [`Percent`, `Normalized` and `Ratio32`](#percent-normalized-and-ratio32)
* [4. Tags](#4-tags)
  * [`Tag`](#tag)
  * [`Tags` and `Name`](#tags-and-name)
  * [`normalize`](#normalize)
* [5. Recorders](#5-recorders)
  * [`Recorder`](#recorder)
  * [`RecorderExt`](#recorderext)
  * [`Observation`](#observation)
  * [`Sample`](#sample)
  * [`Context`](#context)
  * [`Noop`](#noop)
  * [`set_global`, `global` and `global_arc`](#set_global-global-and-global_arc)
  * [`Clock`, `SystemClock` and `XsClock`](#clock-systemclock-and-xsclock)
* [6. Aggregation](#6-aggregation)
  * [`Stats`](#stats)
  * [`StatsConfig`](#statsconfig)
  * [`CollectorId`](#collectorid)
  * [`Refusal` and `RefusalReason`](#refusal-and-refusalreason)
  * [`Histogram`](#histogram)
  * [`bucket_index` and `bucket_bounds`](#bucket_index-and-bucket_bounds)
* [7. Sinks and Batches](#7-sinks-and-batches)
  * [`Sink`](#sink)
  * [`VecSink`](#vecsink)
  * [`Batch`](#batch)
  * [`SeriesBatch`](#seriesbatch)
  * [`Points`](#points)
  * [`Dropped`](#dropped)
* [8. Collectors](#8-collectors)
  * [`Collector`](#collector)
  * [`PollGauge`, `PollState` and `PollTotal`](#pollgauge-pollstate-and-polltotal)
  * [`collect::gauge`, `collect::state` and `collect::total`](#collectgauge-collectstate-and-collecttotal)
  * [`collect::process`](#collectprocess)
  * [`collect::host`](#collecthost)
  * [`collect::tokio`](#collecttokio)
* [9. Error Handling](#9-error-handling)
  * [`SetGlobalError`](#setglobalerror)
  * [`CollectError`](#collecterror)

## 1. Defining Metrics

### The `metric!` macro

```rust
metric! {
  /// Description lines.
  #[unit(bytes)]
  #[tags("a", "b")]
  #[high_res("1ms")]
  pub static NAME: Counter = "metric.name";
}
```

* Any number of definitions per block, at module level. Each expands to `$vis static NAME: Handle` where `Handle` is `Counter`, `Gauge`, `Timer` or `State`, plus an `inventory` submission that adds it to [`registry`](#registry-and-find).
* `#[unit(...)]` takes `none`, `count`, `seconds`, `bytes`, `percent`, `ratio`, `celsius`, `hertz` or a string literal for `Unit::Custom`. Default: `count` for a counter, `seconds` for a timer, `none` otherwise.
* `#[tags(...)]` takes string literals. Default: none. A recording through the handle with a tag not listed panics when `debug_assertions` is on and is recorded otherwise.
* `#[high_res(...)]` takes a string literal accepted by [`parse_duration`](#parse_duration). Default: none.
* Doc comments become `Metric::description`, one entry per line, untrimmed.

### `state_enum!`

```rust
state_enum! {
  pub enum Name { Variant = "label", Other = "other" }
}
```

* Expands to the enum with `Debug, Clone, Copy, PartialEq, Eq, Hash`, an [`EnumState`](#enumstate) impl with `ENUM_NAME = stringify!(Name)` and `From<Name> for StateValue` and `for StatValue`.

### `tags!`

* `tags!()` is `Tags::new()`.
* `tags!("name" => value, ...)` is `vec![Tag::new("name", value), ...]`. Each side is `impl Into<Cow<'static, str>>`.

### `Metric`

`pub struct Metric`, `Debug`. Fields are public.

* `pub name: &'static str`
* `pub kind: Kind`
* `pub unit: Unit`
* `pub description: &'static [&'static str]`: the doc comment lines as written.
* `pub tags: &'static [&'static str]`: the tag names a recording may carry.
* `pub resolution: Option<Duration>`: from `#[high_res]`.
* `pub const fn new(name: &'static str, kind: Kind, description: &'static [&'static str], attrs: &'static [MetricAttr]) -> Metric`: applies `attrs` in order; a later `Unit` or `Tags` replaces an earlier one.
* `pub fn description_text(&self) -> String`: lines trimmed and joined with `\n`.
* `pub fn allows(&self, tag: &str) -> bool`

### `MetricAttr`

`pub enum MetricAttr`, `Debug, Clone, Copy`. `Unit(Unit)`, `Tags(&'static [&'static str])`, `Resolution(Duration)`.

### `Kind`

`pub enum Kind`, `Debug, Clone, Copy, PartialEq, Eq, Hash, Display`. `Counter`, `Gauge`, `Timer`, `State`.

* `pub fn as_str(&self) -> &'static str`: `counter`, `gauge`, `timer`, `state`.

### `Unit`

`pub enum Unit`, `Debug, Clone, Copy, PartialEq, Eq, Hash`. `None`, `Count`, `Seconds`, `Bytes`, `Percent`, `Ratio`, `Celsius`, `Hertz`, `Custom(&'static str)`.

* `pub fn as_str(&self) -> &'static str`: the lowercase name, `""` for `None` and the string for `Custom`.

### `registry` and `find`

* `pub fn registry() -> impl Iterator<Item = &'static Metric>`: every `metric!` definition the linker kept, in no defined order. A definition whose handle the program refers to is always kept. One in a library the program does not refer to can be missing, more often on wasm than on native targets. Two crates may define the same name; both appear. On wasm a `cdylib` fills it at the start of the first exported function that runs.
* `pub fn find(name: &str) -> Option<&'static Metric>`: the first definition with that name.

### `parse_duration`

* `pub const fn parse_duration(s: &str) -> Duration` in `fibre_stats::metric`. Accepts digits followed by `ns`, `us`, `ms` or `s`. Panics on anything else, which is a compile error when called from `metric!`.

## 2. Handles

Every recording method has a form that records on the global recorder and a `_to` form taking `&dyn Recorder`. Both check the tags against the definition when `debug_assertions` is on and read the time from the recorder.

### `Counter`

`pub struct Counter { pub metric: Metric }`.

* `pub const fn new(metric: Metric) -> Self`
* `pub fn incr(&self, tags: Tags)` and `pub fn add(&self, delta: u64, tags: Tags)`
* `pub fn add_to(&self, recorder: &dyn Recorder, delta: u64, tags: Tags)`
* `pub fn total(&self, total: u64, tags: Tags)` and `pub fn total_to(&self, recorder: &dyn Recorder, total: u64, tags: Tags)`: see [`Recorder::total`](#recorder).
* `pub fn poll_total<F: Fn() -> u64 + Send + 'static>(&'static self, tags: Tags, read: F) -> PollTotal<F>`

### `Gauge`

`pub struct Gauge { pub metric: Metric }`.

* `pub const fn new(metric: Metric) -> Self`
* `pub fn set(&self, value: impl Into<NumberValue>, tags: Tags)`
* `pub fn set_to(&self, recorder: &dyn Recorder, value: impl Into<NumberValue>, tags: Tags)`
* `pub fn poll<F: Fn() -> NumberValue + Send + 'static>(&'static self, tags: Tags, read: F) -> PollGauge<F>`

### `Timer`

`pub struct Timer { pub metric: Metric }`.

* `pub const fn new(metric: Metric) -> Self`
* `pub fn record(&self, duration: Duration, tags: Tags)` and `pub fn record_to(&self, recorder: &dyn Recorder, duration: Duration, tags: Tags)`
* `pub fn time<R>(&self, tags: Tags, f: impl FnOnce() -> R) -> R` and `pub fn time_to<R>(&self, recorder: &dyn Recorder, tags: Tags, f: impl FnOnce() -> R) -> R`: records the wall time `f` took.
* `pub fn guard(&self, tags: Tags) -> TimerGuard<'_>` and `pub fn guard_to<'a>(&'a self, recorder: &'a dyn Recorder, tags: Tags) -> TimerGuard<'a>`

### `TimerGuard`

`pub struct TimerGuard<'a>`. Records the time since `guard` on drop, unless discarded.

* `pub fn stop(self)`: records now.
* `pub fn discard(self)`: records nothing.
* `pub fn set_tags(&mut self, tags: Tags)`: replaces the tags recorded on drop. Not checked against the definition.
* `pub fn elapsed(&self) -> Duration`

On `wasm*-unknown-unknown` without the `wasm-bindgen` feature a timer measures with the recorder's clock, so it reads zero on a `Noop`. Everywhere else it uses a monotonic `Instant`.

### `State`

`pub struct State { pub metric: Metric }`.

* `pub const fn new(metric: Metric) -> Self`
* `pub fn set(&self, value: impl Into<StateValue>, tags: Tags)`
* `pub fn set_to(&self, recorder: &dyn Recorder, value: impl Into<StateValue>, tags: Tags)`
* `pub fn poll<F: Fn() -> StateValue + Send + 'static>(&'static self, tags: Tags, read: F) -> PollState<F>`

## 3. Values

### `IntValue`

`pub enum IntValue`, `Debug, Clone, Copy, PartialEq, Eq, Hash, Display`. `I8`, `U8`, `I16`, `U16`, `I32`, `U32`, `I64`, `U64`, `I128`, `U128`, each holding its integer. `From` every integer type and `usize` (as `U64`).

* `pub fn bits(&self) -> u8`
* `pub fn is_signed(&self) -> bool`
* `pub fn to_i64(&self) -> Option<i64>`, `pub fn to_u64(&self) -> Option<u64>`, `pub fn to_i128(&self) -> Option<i128>`: `None` when the value does not fit.

### `NumberValue`

`pub enum NumberValue`, `Debug, Clone, Copy, PartialEq, Eq, Hash, Display`. `Int(IntValue)`, `Count(u64)`, `Percent(Percent)`, `Normalized(Normalized)`, `Ratio32(Ratio32)`. `From` every integer type, `usize`, `IntValue`, `Percent`, `Normalized` and `Ratio32`; integers become `Int`.

* `pub fn count(v: u64) -> Self`
* `pub fn percent(v: f32) -> Option<Self>`: `None` for NaN, negative or 128 and above.
* `pub fn percent_saturating(v: f32) -> Self`: clamps into `Percent`; NaN is zero.
* `pub fn normalized(v: f64) -> Option<Self>`: `None` outside `[0, 1]` or NaN.
* `pub fn ratio(v: f64) -> Option<Self>`: the closest `Ratio32`, `None` when there is none.
* `pub fn ratio_of(numer: i32, denom: i32) -> Self`: panics when `denom` is zero.
* `pub fn encoding(&self) -> Encoding`
* `pub fn to_f64(&self) -> f64`

### `StateValue`

`pub enum StateValue`, `Debug, Clone, Copy, PartialEq, Eq, Hash, Display`. `Bool(bool)`, `Char(char)`, `Enum { enum_name: &'static str, label: &'static str }`. `From<bool>` and `From<char>`.

* `pub fn label(enum_name: &'static str, label: &'static str) -> Self`

### `StatValue`

`pub enum StatValue`, `Debug, Clone, Copy, PartialEq, Eq, Hash, Display`. `Number(NumberValue)`, `State(StateValue)`. `From` every integer type, `bool`, `NumberValue` and `StateValue`.

* `pub fn encoding(&self) -> Encoding`

### `EnumState`

`pub trait EnumState: Copy`.

* `const ENUM_NAME: &'static str`
* `fn label(&self) -> &'static str`: must not change for a variant, since a store keys the variant by it.
* `fn state(&self) -> StateValue`: provided.

### `Encoding`

`pub enum Encoding`, `Debug, Clone, Copy, PartialEq, Eq, Hash, Display`. `Int`, `Count`, `Percent`, `Normalized`, `Ratio32`, `State`, `Histogram`.

* `pub fn as_str(&self) -> &'static str`: the lowercase name.

### `Percent`, `Normalized` and `Ratio32`

* `pub type Percent = fixed::FixedU16<fixed::types::extra::U9>`: 0 to 127.998 in steps of 1/512.
* `pub type Normalized = fixed::FixedU32<fixed::types::extra::U31>`: 0 to 1 in steps of 2^-31 for values built with `NumberValue::normalized`.
* `pub type Ratio32 = num_rational::Rational32`

## 4. Tags

### `Tag`

`pub struct Tag { pub name: Name, pub value: Name }`, `Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord`.

* `pub fn new(name: impl Into<Name>, value: impl Into<Name>) -> Self`

### `Tags` and `Name`

* `pub type Tags = Vec<Tag>`
* `pub type Name = Cow<'static, str>`

### `normalize`

* `pub fn normalize(tags: &mut Tags)` in `fibre_stats::tag`: sorts by name; when a name repeats the last value is kept. `Stats` applies it to every recording after prepending the common tags.

## 5. Recorders

### `Recorder`

`pub trait Recorder: Send + Sync`.

* `fn now_ns(&self) -> u64`: nanoseconds since the Unix epoch.
* `fn record(&self, observation: Observation)`
* `fn total(&self, name: Name, tags: Tags, total: u64)`: the first total for a series sets a baseline and records nothing; each later total records `total - previous` as a counter delta; a total below the previous one records `total`.

### `RecorderExt`

`pub trait RecorderExt: Recorder`, implemented for every `R: Recorder + ?Sized`. Each method records at `now_ns()` with `resolution: None` and no tag check.

* `fn observe(&self, name: impl Into<Name>, kind: Kind, tags: Tags, sample: Sample)`
* `fn incr(&self, name: impl Into<Name>, tags: Tags)`
* `fn add(&self, name: impl Into<Name>, tags: Tags, delta: u64)`
* `fn gauge(&self, name: impl Into<Name>, tags: Tags, value: impl Into<NumberValue>)`
* `fn timer(&self, name: impl Into<Name>, tags: Tags, duration: Duration)`
* `fn state(&self, name: impl Into<Name>, tags: Tags, value: impl Into<StateValue>)`

### `Observation`

`pub struct Observation`, `Debug, Clone, PartialEq, Eq`. Fields are public.

* `pub name: Name`
* `pub kind: Kind`
* `pub tags: Tags`
* `pub at_ns: u64`: nanoseconds since the Unix epoch.
* `pub sample: Sample`
* `pub resolution: Option<Duration>`: when set and the `high_res` feature is on, `Stats` keeps the exact sample as well.

### `Sample`

`pub enum Sample`, `Debug, Clone, PartialEq, Eq`. `Count(u64)`, `Number(NumberValue)`, `State(StateValue)`, `Duration(Duration)`.

* `pub fn encoding(&self) -> Encoding`: `Count`, the number's encoding, `State` or `Histogram`.
* `pub fn fits(&self, kind: Kind) -> bool`: `Count` fits `Counter`, `Number` fits `Gauge`, `State` fits `State`, `Duration` fits `Timer`.

### `Context`

`pub struct Context`, implements `Recorder`. Prepends its tags to every recording and total; a tag on the recording with the same name wins after normalization.

* `pub fn new(parent: Arc<dyn Recorder>, tags: Tags) -> Self`
* `pub fn tags(&self) -> &Tags`

### `Noop`

`pub struct Noop`, `Debug, Default, Clone, Copy`, implements `Recorder`. Records nothing. `now_ns` returns 0.

### `set_global`, `global` and `global_arc`

* `pub fn set_global(recorder: Arc<dyn Recorder>) -> Result<(), SetGlobalError>`: succeeds once per process.
* `pub fn global() -> &'static dyn Recorder`: the installed recorder. A `Noop` when none is installed.
* `pub fn global_arc() -> Option<Arc<dyn Recorder>>`

### `Clock`, `SystemClock` and `XsClock`

* `pub trait Clock: Send + Sync { fn now_ns(&self) -> u64; }`
* `pub struct SystemClock`, `Debug, Default, Clone, Copy`: `SystemTime::now()` since the Unix epoch; 0 when the clock is before it. On `wasm*-unknown-unknown` it needs the `wasm-bindgen` feature, which reads `Date.now()`. It does not exist without that feature.
* `pub struct XsClock(pub Arc<dyn xs_foundation::time::Clock>)`, feature `xs_foundation`: `now().as_nano()`.

## 6. Aggregation

### `Stats`

`pub struct Stats`, `Clone`, implements `Recorder`. Every clone shares one book. Threads are started by `start` and `add_collector` and hold only a weak reference, so dropping the last clone flushes what is left and lets the threads exit.

* `pub fn new(config: StatsConfig) -> Stats`
* `pub fn config(&self) -> &StatsConfig`
* `pub fn start(&self, sink: Arc<dyn Sink>)`: sets the sink and starts the flush thread `fibre-stats-flush`, which calls `flush` every `flush_interval`. A second call only replaces the sink. Does not exist on wasm targets.
* `pub fn set_sink(&self, sink: Arc<dyn Sink>)`: sets the sink without a thread.
* `pub fn flush(&self)`: `drain(false)` to the sink, skipped when empty or when no sink is set.
* `pub fn drain(&self, include_open: bool) -> Batch`: removes every window that ended before `now_ns()` plus every buffered high resolution sample. With `include_open` the current window goes too. Points are in window order per series.
* `pub fn stop(&self)`: stops both threads, joins them unless called from one of them, then hands `drain(true)` to the sink.
* `pub fn add_collector(&self, interval: Duration, collector: impl Collector) -> CollectorId`: runs on the thread `fibre-stats-collect`, first after `interval`. On wasm targets no thread starts and `collect_due` runs it. The schedule reads `StatsConfig::clock`; a due time more than one interval ahead after the clock steps back is pulled in to one interval. The collector runs with the collectors lock held, so a collector that adds or removes collectors deadlocks.
* `pub fn add_collector_with_tags(&self, tags: Tags, interval: Duration, collector: impl Collector) -> CollectorId`: the collector records through a `Context` carrying `tags`.
* `pub fn remove_collector(&self, id: CollectorId) -> bool`
* `pub fn collect_now(&self)`: runs every collector once on the calling thread and resets its schedule.
* `pub fn collect_due(&self)`: runs the collectors that are due on the calling thread.
* `pub fn refused(&self) -> u64` and `pub fn last_refusal(&self) -> Option<Refusal>`
* `pub fn context(&self, tags: Tags) -> Arc<dyn Recorder>`

### `StatsConfig`

`pub struct StatsConfig`. Fields are public. `Default` uses `SystemClock` and does not exist on `wasm*-unknown-unknown` without the `wasm-bindgen` feature.

* `pub fn with_clock(clock: Arc<dyn Clock>) -> Self`: the defaults with this clock.

* `pub common_tags: Tags`: joined into every series key and reported once per batch. Default empty.
* `pub resolution: Duration`: the window. Default 1 s. Below 1 ns it is treated as 1 ns.
* `pub flush_interval: Duration`: default 10 s.
* `pub high_res_buffer: usize`: exact samples kept per high resolution series between flushes. Default 65536; 0 is treated as 1.
* `pub clock: Arc<dyn Clock>`: where timestamps and the collector schedule read the time.

### `CollectorId`

`pub struct CollectorId`, `Debug, Clone, Copy, PartialEq, Eq, Hash`.

### `Refusal` and `RefusalReason`

* `pub struct Refusal { pub name: Name, pub tags: Tags, pub reason: RefusalReason }`, `Debug, Clone, PartialEq, Eq`. `tags` are the normalized series tags, common tags included.
* `pub enum RefusalReason`, `Debug, Clone, Copy, PartialEq, Eq`: `SampleKind { kind, encoding }`, `KindChanged { was, now }`, `EncodingChanged { was, now }`.

### `Histogram`

`pub struct Histogram`, `Debug, Clone, PartialEq, Eq, Default`. Durations in nanoseconds in exponential buckets, eight per power of two.

* `pub fn new() -> Self`
* `pub fn add(&mut self, duration: Duration)` and `pub fn add_ns(&mut self, ns: u64)`: a duration above `u64::MAX` ns is clamped.
* `pub fn merge(&mut self, other: &Histogram)`: bucket by bucket; associative and commutative.
* `pub fn count(&self) -> u64`, `pub fn is_empty(&self) -> bool`
* `pub fn sum(&self) -> Duration`: saturating.
* `pub fn min(&self) -> Duration` and `pub fn max(&self) -> Duration`: zero when empty.
* `pub fn mean(&self) -> Option<Duration>`
* `pub fn quantile(&self, q: f64) -> Option<Duration>`: `q` clamped to `[0, 1]`; the midpoint of the bucket holding the `ceil(q * count)`th value, clamped to `[min, max]`. Within 6.25% of the true value.
* `pub fn buckets(&self) -> &[(u16, u32)]`: `(index, count)` sorted by index. A bucket count saturates at `u32::MAX`.
* `pub fn from_parts(count: u64, sum_ns: u64, min_ns: u64, max_ns: u64, buckets: Vec<(u16, u32)>) -> Self`

### `bucket_index` and `bucket_bounds`

In `fibre_stats::histogram`.

* `pub const SUB_BUCKET_BITS: u32 = 3`
* `pub fn bucket_index(ns: u64) -> u16`: values below 8 are their own bucket; otherwise `octave * 8 + sub` where `octave` is the position of the leading one and `sub` the three bits below it. Indexes 8 to 23 are never produced. The highest index is 511.
* `pub fn bucket_bounds(index: u16) -> (u64, u64)`: inclusive bounds; `(0, 0)` for an index that is never produced.

## 7. Sinks and Batches

### `Sink`

`pub trait Sink: Send + Sync { fn push(&self, batch: Batch); }`. Implemented for every `F: Fn(Batch) + Send + Sync`. Called on the flush thread. `stop`, `flush` and the final flush on drop call it on the caller's thread.

### `VecSink`

`pub struct VecSink`, `Default`, implements `Sink`. Keeps every batch.

* `pub fn new() -> Self`
* `pub fn take(&self) -> Vec<Batch>`: removes and returns everything received.
* `pub fn len(&self) -> usize` and `pub fn is_empty(&self) -> bool`

### `Batch`

`pub struct Batch`, `Debug, Clone, Default, PartialEq`. Fields are public.

* `pub common_tags: Tags`: on every series, not repeated in `SeriesBatch::tags`.
* `pub series: Vec<SeriesBatch>`
* `pub dropped: Vec<Dropped>`
* `pub fn is_empty(&self) -> bool`
* `pub fn series(&self, name: &str, tags: &Tags) -> Option<&SeriesBatch>`: exact tag set, any order.

### `SeriesBatch`

`pub struct SeriesBatch`, `Debug, Clone, PartialEq`. Fields are public.

* `pub name: Name`
* `pub kind: Kind`
* `pub encoding: Encoding`: fixed by the series' first recording.
* `pub tags: Tags`: normalized, without the common tags.
* `pub resolution: Duration`: the window for aggregated points. For `Points::Samples` it is the declared resolution.
* `pub points: Points`

### `Points`

`pub enum Points`, `Debug, Clone, PartialEq`. Every timestamp is nanoseconds since the Unix epoch. For aggregated points it is the start of the window; for samples it is the recording's own time.

* `Counts(Vec<(u64, u64)>)`: counter deltas summed per window.
* `Numbers(Vec<(u64, NumberValue)>)`: the last gauge level per window.
* `States(Vec<(u64, StateValue)>)`: the last state per window.
* `Histograms(Vec<(u64, Histogram)>)`: timer durations per window.
* `Samples(Vec<(u64, Sample)>)`: exact samples of a high resolution series.
* `pub fn len(&self) -> usize` and `pub fn is_empty(&self) -> bool`
* `pub fn counts(&self) -> Option<&[(u64, u64)]>`, `numbers`, `states`, `histograms`, `samples`: the variant's points. `None` for another variant.

### `Dropped`

`pub struct Dropped { pub name: Name, pub tags: Tags, pub count: u64, pub from_ns: u64, pub to_ns: u64 }`, `Debug, Clone, PartialEq, Eq`. High resolution samples of one series dropped since the last flush because the buffer was full; `from_ns` and `to_ns` are the times of the first and last dropped sample.

## 8. Collectors

### `Collector`

`pub trait Collector: Send + 'static { fn collect(&mut self, recorder: &dyn Recorder); }`. Implemented for every `F: FnMut(&dyn Recorder) + Send + 'static`.

### `PollGauge`, `PollState` and `PollTotal`

`pub struct PollGauge<F>`, `PollState<F>`, `PollTotal<F>` in `fibre_stats::collect`, each implementing `Collector` for the matching `F: Fn() -> NumberValue`, `Fn() -> StateValue` or `Fn() -> u64`, all `Send + 'static`.

* `pub fn new(name: Name, tags: Tags, read: F) -> Self`
* `collect` records the function's value at `recorder.now_ns()`; `PollTotal` calls `Recorder::total`.

### `collect::gauge`, `collect::state` and `collect::total`

* `pub fn gauge<F>(name: impl Into<Name>, tags: Tags, read: F) -> PollGauge<F>`
* `pub fn state<F>(name: impl Into<Name>, tags: Tags, read: F) -> PollState<F>`
* `pub fn total<F>(name: impl Into<Name>, tags: Tags, read: F) -> PollTotal<F>`

### `collect::process`

Feature `collect-process`. Not on wasm targets.

* `pub fn process(pid: u32) -> Result<ProcessCollector, CollectError>`
* `pub struct ProcessCollector`, implements `Collector`. `pub fn pid(&self) -> u32`.
* Each run refreshes the one process and records, with no tags of its own: `process.alive` (State, Bool), `process.cpu_pct` (Gauge, percent, CPU over every core so 100 is the machine; zero on the first run), `process.memory_bytes`, `process.virtual_memory_bytes` (Gauge, bytes), `process.threads` (Gauge, Linux only), `process.open_fds` (Gauge, only where the platform reports it), `process.disk.read_bytes` and `process.disk.written_bytes` (Counter, bytes since the previous run), `process.run_time_secs` (Gauge, seconds). When the process is gone only `process.alive` is recorded, as `false`.

### `collect::host`

Feature `collect-host`. Not on wasm targets.

* `pub fn host() -> Result<HostCollector, CollectError>`
* `pub struct HostCollector`, implements `Collector`.
* Each run records: `host.memory.total_bytes`, `used_bytes`, `free_bytes`, `available_bytes`, `host.swap.total_bytes`, `used_bytes`, `free_bytes` (Gauge, bytes); `host.cpu.usage_pct` (Gauge, percent, zero on the first run); `host.cpu.core.usage_pct` tagged `cpu`; `host.cpu.load_1`, `load_5`, `load_15` (Gauge, ratio); `host.uptime_secs`; `host.net.recv_bytes`, `sent_bytes`, `recv_packets`, `sent_packets`, `recv_errors`, `sent_errors` tagged `iface` (Counter, since the previous run); `host.disk.total_bytes`, `free_bytes` (Gauge, bytes) and `free_pct` tagged `disk` and `mount`; `host.temp.celsius`, `max_celsius`, `critical_celsius` tagged `component` (Gauge, Ratio32), each only when the component reports a finite value.

### `collect::tokio`

Feature `collect-tokio`. Not on wasm targets.

* `pub fn tokio(handle: tokio::runtime::Handle) -> TokioCollector`
* `pub struct TokioCollector`, implements `Collector`.
* Each run records `tokio.workers`, `tokio.tasks.alive`, `tokio.queue.global` (Gauge) and, from the second run on and only where the target has 64-bit atomics, `tokio.worker.busy_pct` (Gauge, percent of wall time since the previous run) and `tokio.worker.parks` (Counter) tagged `worker`.

## 9. Error Handling

Recording never returns an error. A recording the book does not take is a refusal: counted in `Stats::refused`, described by `Stats::last_refusal` and otherwise dropped.

### `SetGlobalError`

`pub struct SetGlobalError(pub Arc<dyn Recorder>)`, `Debug, Display, Error`. Returned by `set_global` when a global recorder is already set; holds the recorder that was not installed.

### `CollectError`

`pub enum CollectError`, `Debug, Clone, PartialEq, Eq, Display, Error`.

* `Unavailable(String)`: `collect::process` or `collect::host` could not read what it needs. The string is the underlying error's debug form.
