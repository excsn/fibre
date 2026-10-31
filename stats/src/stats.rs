//! The recorder that aggregates.
//!
//! [`Stats`] keeps one aggregate per series per window: counters are summed,
//! gauges and states keep their last value and timers become histograms. A
//! flush closes every window that has ended and hands the points to a
//! [`Sink`] as one [`Batch`]. Collectors run on their own thread at their own
//! interval and record like anything else.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use crate::collect::Collector;
use crate::histogram::Histogram;
use crate::metric::Kind;
use crate::recorder::{Clock, Context, Observation, Recorder, Sample};
use crate::sink::{Batch, Dropped, Points, SeriesBatch, Sink};
use crate::tag::{Name, Tags, normalize};
use crate::value::{Encoding, NumberValue, StateValue};

/// How a [`Stats`] aggregates and flushes.
pub struct StatsConfig {
  /// Tags added to every series. Sent once per batch rather than on each series.
  pub common_tags: Tags,
  /// The window each point covers. One second by default.
  pub resolution: Duration,
  /// How often closed windows are handed to the sink once `start` has been called. Ten seconds by default.
  pub flush_interval: Duration,
  /// Exact samples kept per high resolution series between flushes. When it is
  /// full the oldest sample is dropped and counted. 65536 by default.
  pub high_res_buffer: usize,
  /// Where timestamps and the collector schedule read the time.
  pub clock: Arc<dyn Clock>,
}

impl StatsConfig {
  /// The defaults with this clock. `wasm*-unknown-unknown` without the
  /// `wasm-bindgen` feature has no `Default`, so this is how a host starts there.
  pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
    StatsConfig {
      common_tags: Vec::new(),
      resolution: Duration::from_secs(1),
      flush_interval: Duration::from_secs(10),
      high_res_buffer: 65_536,
      clock,
    }
  }
}

#[cfg(any(not(all(target_family = "wasm", target_os = "unknown")), feature = "wasm-bindgen"))]
impl Default for StatsConfig {
  fn default() -> Self {
    StatsConfig::with_clock(Arc::new(crate::recorder::SystemClock))
  }
}

/// Identifies a collector added to a [`Stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollectorId(u64);

/// A recording the book did not accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
  pub name: Name,
  pub tags: Tags,
  pub reason: RefusalReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
  /// The sample does not fit the observation's kind, such as a number on a counter.
  SampleKind { kind: Kind, encoding: Encoding },
  /// The series was first recorded with another kind.
  KindChanged { was: Kind, now: Kind },
  /// The series was first recorded with another encoding.
  EncodingChanged { was: Encoding, now: Encoding },
}

/// The aggregating recorder. Cheap to clone; every clone shares one book.
#[derive(Clone)]
pub struct Stats {
  inner: Arc<Inner>,
}

struct Inner {
  config: StatsConfig,
  book: Mutex<Book>,
  sink: Mutex<Option<Arc<dyn Sink>>>,
  shutdown: Arc<Shutdown>,
  flusher: Mutex<Option<JoinHandle<()>>>,
  schedule: Arc<Schedule>,
  collect_thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SeriesKey {
  name: Name,
  tags: Tags,
}

#[derive(Clone, Copy)]
struct Header {
  kind: Kind,
  encoding: Encoding,
}

enum Agg {
  Count(u64),
  Number(NumberValue),
  State(StateValue),
  Hist(Histogram),
}

impl Agg {
  fn start(sample: &Sample) -> Agg {
    match sample {
      Sample::Count(n) => Agg::Count(*n),
      Sample::Number(v) => Agg::Number(*v),
      Sample::State(s) => Agg::State(*s),
      Sample::Duration(d) => {
        let mut histogram = Histogram::new();
        histogram.add(*d);
        Agg::Hist(histogram)
      }
    }
  }

  fn absorb(&mut self, sample: &Sample) {
    match (self, sample) {
      (Agg::Count(total), Sample::Count(n)) => *total = total.saturating_add(*n),
      (Agg::Number(level), Sample::Number(n)) => *level = *n,
      (Agg::State(state), Sample::State(s)) => *state = *s,
      (Agg::Hist(histogram), Sample::Duration(d)) => histogram.add(*d),
      _ => {}
    }
  }
}

struct HighRes {
  resolution: Duration,
  samples: VecDeque<(u64, Sample)>,
  dropped: u64,
  dropped_from: u64,
  dropped_to: u64,
}

#[cfg_attr(not(feature = "high_res"), allow(dead_code))]
impl HighRes {
  fn new(resolution: Duration) -> Self {
    HighRes {
      resolution,
      samples: VecDeque::new(),
      dropped: 0,
      dropped_from: 0,
      dropped_to: 0,
    }
  }

  fn push(&mut self, at_ns: u64, sample: Sample, cap: usize) {
    if self.samples.len() >= cap
      && let Some((dropped_at, _)) = self.samples.pop_front()
    {
      if self.dropped == 0 {
        self.dropped_from = dropped_at;
      }
      self.dropped_to = dropped_at;
      self.dropped += 1;
    }
    self.samples.push_back((at_ns, sample));
  }
}

#[derive(Default)]
struct Book {
  headers: HashMap<SeriesKey, Header>,
  windows: BTreeMap<u64, HashMap<SeriesKey, Agg>>,
  totals: HashMap<SeriesKey, u64>,
  high_res: HashMap<SeriesKey, HighRes>,
  refused: u64,
  last_refusal: Option<Refusal>,
}

impl Book {
  fn refuse(&mut self, key: SeriesKey, reason: RefusalReason) {
    self.refused += 1;
    self.last_refusal = Some(Refusal {
      name: key.name,
      tags: key.tags,
      reason,
    });
  }
}

struct Shutdown {
  stopped: Mutex<bool>,
  cvar: Condvar,
}

impl Shutdown {
  fn new() -> Self {
    Shutdown {
      stopped: Mutex::new(false),
      cvar: Condvar::new(),
    }
  }

  /// Sleeps up to `dur`, waking early on `signal`. Returns whether to stop.
  #[cfg(not(target_family = "wasm"))]
  fn sleep_or_stopped(&self, dur: Duration) -> bool {
    let mut stopped = self.stopped.lock();
    if *stopped {
      return true;
    }
    self.cvar.wait_for(&mut stopped, dur);
    *stopped
  }

  fn signal(&self) {
    *self.stopped.lock() = true;
    self.cvar.notify_all();
  }
}

struct Schedule {
  entries: Mutex<Vec<Entry>>,
  cvar: Condvar,
  stopped: AtomicBool,
  next_id: AtomicU64,
}

impl Schedule {
  fn signal_stop(&self) {
    let _entries = self.entries.lock();
    self.stopped.store(true, Ordering::Release);
    self.cvar.notify_all();
  }
}

struct Entry {
  id: CollectorId,
  interval_ns: u64,
  due_ns: u64,
  tags: Tags,
  collector: Box<dyn Collector>,
}

impl Stats {
  pub fn new(config: StatsConfig) -> Stats {
    Stats {
      inner: Arc::new(Inner {
        config,
        book: Mutex::new(Book::default()),
        sink: Mutex::new(None),
        shutdown: Arc::new(Shutdown::new()),
        flusher: Mutex::new(None),
        schedule: Arc::new(Schedule {
          entries: Mutex::new(Vec::new()),
          cvar: Condvar::new(),
          stopped: AtomicBool::new(false),
          next_id: AtomicU64::new(0),
        }),
        collect_thread: Mutex::new(None),
      }),
    }
  }

  pub fn config(&self) -> &StatsConfig {
    &self.inner.config
  }

  /// Sets the sink and starts the flush thread. Calling it again only replaces
  /// the sink. Wasm targets have no threads, so there it is `set_sink` and `flush`.
  #[cfg(not(target_family = "wasm"))]
  pub fn start(&self, sink: Arc<dyn Sink>) {
    *self.inner.sink.lock() = Some(sink);
    let mut flusher = self.inner.flusher.lock();
    if flusher.is_none() {
      *flusher = Some(spawn_flusher(
        Arc::downgrade(&self.inner),
        self.inner.shutdown.clone(),
        self.inner.config.flush_interval,
      ));
    }
  }

  /// Sets the sink without starting a thread. `flush` then hands batches over.
  pub fn set_sink(&self, sink: Arc<dyn Sink>) {
    *self.inner.sink.lock() = Some(sink);
  }

  /// Hands every closed window to the sink now.
  pub fn flush(&self) {
    self.inner.flush();
  }

  /// Takes every closed window out of the book as a batch. With `include_open`
  /// the window still being written is taken as well.
  pub fn drain(&self, include_open: bool) -> Batch {
    self.inner.drain(include_open)
  }

  /// Stops the flush and collect threads, waits for them and hands everything
  /// left, open windows included, to the sink.
  pub fn stop(&self) {
    let inner = &self.inner;
    inner.shutdown.signal();
    inner.schedule.signal_stop();
    join_unless_self(inner.flusher.lock().take());
    join_unless_self(inner.collect_thread.lock().take());
    inner.final_flush();
  }

  /// Runs `collector` every `interval` on the collect thread, first after one
  /// interval. On wasm targets there is no thread and the host calls
  /// `collect_due`. A collector must not add or remove collectors.
  pub fn add_collector(&self, interval: Duration, collector: impl Collector) -> CollectorId {
    self.add_collector_with_tags(Vec::new(), interval, collector)
  }

  /// `add_collector` with tags added to everything the collector records.
  pub fn add_collector_with_tags(&self, tags: Tags, interval: Duration, collector: impl Collector) -> CollectorId {
    let schedule = &self.inner.schedule;
    let id = CollectorId(schedule.next_id.fetch_add(1, Ordering::Relaxed) + 1);
    let interval_ns = u64::try_from(interval.as_nanos()).unwrap_or(u64::MAX);
    let due_ns = self.now_ns().saturating_add(interval_ns);
    schedule.entries.lock().push(Entry {
      id,
      interval_ns,
      due_ns,
      tags,
      collector: Box::new(collector),
    });
    schedule.cvar.notify_all();
    self.ensure_collect_thread();
    id
  }

  #[cfg(not(target_family = "wasm"))]
  fn ensure_collect_thread(&self) {
    let mut thread = self.inner.collect_thread.lock();
    if thread.is_none() {
      *thread = Some(spawn_collect(
        Arc::downgrade(&self.inner),
        self.inner.schedule.clone(),
        self.inner.config.clock.clone(),
      ));
    }
  }

  #[cfg(target_family = "wasm")]
  fn ensure_collect_thread(&self) {}

  /// Removes a collector. Returns whether it was present.
  pub fn remove_collector(&self, id: CollectorId) -> bool {
    let mut entries = self.inner.schedule.entries.lock();
    let before = entries.len();
    entries.retain(|e| e.id != id);
    entries.len() != before
  }

  /// Runs every collector once on the calling thread, whether or not it is due.
  pub fn collect_now(&self) {
    let mut entries = self.inner.schedule.entries.lock();
    let now = self.now_ns();
    for entry in entries.iter_mut() {
      entry.due_ns = now;
    }
    run_due(&mut entries, self, now);
  }

  /// Runs every collector that is due on the calling thread. This is what a
  /// host without threads calls from its own timer.
  pub fn collect_due(&self) {
    let mut entries = self.inner.schedule.entries.lock();
    let now = self.now_ns();
    run_due(&mut entries, self, now);
  }

  /// How many recordings were refused since the book was created.
  pub fn refused(&self) -> u64 {
    self.inner.book.lock().refused
  }

  pub fn last_refusal(&self) -> Option<Refusal> {
    self.inner.book.lock().last_refusal.clone()
  }

  /// A recorder that adds `tags` to everything recorded through it.
  pub fn context(&self, tags: Tags) -> Arc<dyn Recorder> {
    Arc::new(Context::new(Arc::new(self.clone()), tags))
  }
}

impl Recorder for Stats {
  fn now_ns(&self) -> u64 {
    self.inner.config.clock.now_ns()
  }

  fn record(&self, observation: Observation) {
    self.inner.record(observation);
  }

  fn total(&self, name: Name, tags: Tags, total: u64) {
    self.inner.total(name, tags, total);
  }
}

impl Inner {
  fn resolution_ns(&self) -> u64 {
    u64::try_from(self.config.resolution.as_nanos())
      .unwrap_or(u64::MAX)
      .max(1)
  }

  fn key(&self, name: Name, mut tags: Tags) -> SeriesKey {
    let mut all = self.config.common_tags.clone();
    all.append(&mut tags);
    normalize(&mut all);
    SeriesKey { name, tags: all }
  }

  fn record(&self, observation: Observation) {
    let Observation {
      name,
      kind,
      tags,
      at_ns,
      sample,
      resolution,
    } = observation;
    let key = self.key(name, tags);
    self.ingest(key, kind, at_ns, sample, resolution);
  }

  fn ingest(&self, key: SeriesKey, kind: Kind, at_ns: u64, sample: Sample, resolution: Option<Duration>) {
    let encoding = sample.encoding();
    let mut guard = self.book.lock();
    let book = &mut *guard;

    if !sample.fits(kind) {
      book.refuse(key, RefusalReason::SampleKind { kind, encoding });
      return;
    }
    match book.headers.get(&key).copied() {
      Some(header) if header.kind != kind => {
        book.refuse(
          key,
          RefusalReason::KindChanged {
            was: header.kind,
            now: kind,
          },
        );
        return;
      }
      Some(header) if header.encoding != encoding => {
        book.refuse(
          key,
          RefusalReason::EncodingChanged {
            was: header.encoding,
            now: encoding,
          },
        );
        return;
      }
      Some(_) => {}
      None => {
        book.headers.insert(key.clone(), Header { kind, encoding });
      }
    }

    let window = at_ns / self.resolution_ns();
    let series = book.windows.entry(window).or_default();
    match series.get_mut(&key) {
      Some(agg) => agg.absorb(&sample),
      None => {
        series.insert(key.clone(), Agg::start(&sample));
      }
    }

    self.keep_high_res(book, key, at_ns, sample, resolution);
  }

  #[cfg(feature = "high_res")]
  fn keep_high_res(&self, book: &mut Book, key: SeriesKey, at_ns: u64, sample: Sample, resolution: Option<Duration>) {
    let Some(resolution) = resolution else {
      return;
    };
    let cap = self.config.high_res_buffer.max(1);
    let high = book
      .high_res
      .entry(key)
      .or_insert_with(|| HighRes::new(resolution));
    high.push(at_ns, sample, cap);
  }

  #[cfg(not(feature = "high_res"))]
  fn keep_high_res(&self, _book: &mut Book, _key: SeriesKey, _at_ns: u64, _sample: Sample, _resolution: Option<Duration>) {}

  fn total(&self, name: Name, tags: Tags, total: u64) {
    let key = self.key(name, tags);
    let delta = {
      let mut book = self.book.lock();
      match book.totals.insert(key.clone(), total) {
        None => return,
        Some(previous) if total >= previous => total - previous,
        Some(_) => total,
      }
    };
    let at_ns = self.config.clock.now_ns();
    self.ingest(key, Kind::Counter, at_ns, Sample::Count(delta), None);
  }

  fn drain(&self, include_open: bool) -> Batch {
    let res_ns = self.resolution_ns();
    let current = self.config.clock.now_ns() / res_ns;

    let (closed, headers, high) = {
      let mut guard = self.book.lock();
      let book = &mut *guard;
      let closed = if include_open {
        std::mem::take(&mut book.windows)
      } else {
        let open = book.windows.split_off(&current);
        std::mem::replace(&mut book.windows, open)
      };
      let mut high = Vec::new();
      for (key, series) in book.high_res.iter_mut() {
        if series.samples.is_empty() && series.dropped == 0 {
          continue;
        }
        let samples: Vec<(u64, Sample)> = series.samples.drain(..).collect();
        let dropped = (series.dropped, series.dropped_from, series.dropped_to);
        series.dropped = 0;
        high.push((key.clone(), series.resolution, samples, dropped));
      }
      (closed, book.headers.clone(), high)
    };

    let common = &self.config.common_tags;
    let mut batch = Batch {
      common_tags: common.clone(),
      series: Vec::new(),
      dropped: Vec::new(),
    };
    let mut index: HashMap<SeriesKey, usize> = HashMap::new();

    for (window, series) in closed {
      let at_ns = window.saturating_mul(res_ns);
      for (key, agg) in series {
        let Some(header) = headers.get(&key).copied() else {
          continue;
        };
        let at = match index.get(&key) {
          Some(at) => *at,
          None => {
            batch.series.push(SeriesBatch {
              name: key.name.clone(),
              kind: header.kind,
              encoding: header.encoding,
              tags: strip_common(&key.tags, common),
              resolution: self.config.resolution,
              points: empty_points(header.kind),
            });
            let at = batch.series.len() - 1;
            index.insert(key, at);
            at
          }
        };
        push_point(&mut batch.series[at].points, at_ns, agg);
      }
    }

    for (key, resolution, samples, (count, from_ns, to_ns)) in high {
      let Some(header) = headers.get(&key).copied() else {
        continue;
      };
      if count > 0 {
        batch.dropped.push(Dropped {
          name: key.name.clone(),
          tags: strip_common(&key.tags, common),
          count,
          from_ns,
          to_ns,
        });
      }
      if !samples.is_empty() {
        batch.series.push(SeriesBatch {
          name: key.name,
          kind: header.kind,
          encoding: header.encoding,
          tags: strip_common(&key.tags, common),
          resolution,
          points: Points::Samples(samples),
        });
      }
    }

    batch
  }

  fn flush(&self) {
    let sink = self.sink.lock().clone();
    if let Some(sink) = sink {
      let batch = self.drain(false);
      if !batch.is_empty() {
        sink.push(batch);
      }
    }
  }

  fn final_flush(&self) {
    let sink = self.sink.lock().clone();
    if let Some(sink) = sink {
      let batch = self.drain(true);
      if !batch.is_empty() {
        sink.push(batch);
      }
    }
  }
}

impl Drop for Inner {
  fn drop(&mut self) {
    self.shutdown.signal();
    self.schedule.signal_stop();
    self.final_flush();
  }
}

fn strip_common(tags: &Tags, common: &Tags) -> Tags {
  tags.iter().filter(|t| !common.contains(t)).cloned().collect()
}

fn empty_points(kind: Kind) -> Points {
  match kind {
    Kind::Counter => Points::Counts(Vec::new()),
    Kind::Gauge => Points::Numbers(Vec::new()),
    Kind::State => Points::States(Vec::new()),
    Kind::Timer => Points::Histograms(Vec::new()),
  }
}

fn push_point(points: &mut Points, at_ns: u64, agg: Agg) {
  match (points, agg) {
    (Points::Counts(v), Agg::Count(n)) => v.push((at_ns, n)),
    (Points::Numbers(v), Agg::Number(n)) => v.push((at_ns, n)),
    (Points::States(v), Agg::State(s)) => v.push((at_ns, s)),
    (Points::Histograms(v), Agg::Hist(h)) => v.push((at_ns, h)),
    _ => {}
  }
}

fn join_unless_self(handle: Option<JoinHandle<()>>) {
  if let Some(handle) = handle
    && handle.thread().id() != std::thread::current().id()
  {
    let _ = handle.join();
  }
}

#[cfg(not(target_family = "wasm"))]
fn spawn_flusher(weak: std::sync::Weak<Inner>, shutdown: Arc<Shutdown>, interval: Duration) -> JoinHandle<()> {
  std::thread::Builder::new()
    .name("fibre-stats-flush".to_string())
    .spawn(move || {
      loop {
        if shutdown.sleep_or_stopped(interval) {
          break;
        }
        let Some(inner) = weak.upgrade() else {
          break;
        };
        inner.flush();
      }
    })
    .expect("spawn fibre-stats-flush thread")
}

#[cfg(not(target_family = "wasm"))]
fn spawn_collect(weak: std::sync::Weak<Inner>, schedule: Arc<Schedule>, clock: Arc<dyn Clock>) -> JoinHandle<()> {
  std::thread::Builder::new()
    .name("fibre-stats-collect".to_string())
    .spawn(move || {
      loop {
        let mut entries = schedule.entries.lock();
        if schedule.stopped.load(Ordering::Acquire) {
          break;
        }
        let now = clock.now_ns();
        let wait = entries
          .iter()
          .map(|e| e.due_ns.saturating_sub(now).min(e.interval_ns))
          .min();
        match wait {
          None => {
            schedule.cvar.wait(&mut entries);
            continue;
          }
          Some(ns) if ns > 0 => {
            schedule.cvar.wait_for(&mut entries, Duration::from_nanos(ns));
            let now = clock.now_ns();
            if !entries.iter().any(|e| e.due_ns <= now) {
              continue;
            }
          }
          Some(_) => {}
        }
        // The last Stats may drop here and Inner::drop takes this lock.
        drop(entries);
        let Some(inner) = weak.upgrade() else {
          break;
        };
        Stats { inner }.collect_due();
      }
    })
    .expect("spawn fibre-stats-collect thread")
}

fn run_due(entries: &mut [Entry], stats: &Stats, now_ns: u64) {
  for entry in entries.iter_mut() {
    let next_ns = now_ns.saturating_add(entry.interval_ns);
    if entry.due_ns > next_ns {
      // The clock stepped back. Without this the collector waits out the step.
      entry.due_ns = next_ns;
    }
    if entry.due_ns > now_ns {
      continue;
    }
    if entry.tags.is_empty() {
      entry.collector.collect(stats);
    } else {
      let context = Context::new(Arc::new(stats.clone()), entry.tags.clone());
      entry.collector.collect(&context);
    }
    entry.due_ns = next_ns;
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::recorder::RecorderExt;
  use crate::sink::VecSink;
  use crate::tag::Tag;
  use crate::tags;

  struct FakeClock(AtomicU64);

  impl Clock for FakeClock {
    fn now_ns(&self) -> u64 {
      self.0.load(Ordering::Relaxed)
    }
  }

  const SECOND: u64 = 1_000_000_000;

  fn stats_with(clock: &Arc<FakeClock>, common_tags: Tags) -> Stats {
    Stats::new(StatsConfig {
      common_tags,
      clock: clock.clone(),
      high_res_buffer: 2,
      ..StatsConfig::default()
    })
  }

  fn clock_at(ns: u64) -> Arc<FakeClock> {
    Arc::new(FakeClock(AtomicU64::new(ns)))
  }

  #[test]
  fn counters_sum_per_window_and_open_window_waits() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    stats.add("req", vec![], 2);
    stats.add("req", vec![], 3);
    clock.0.store(2 * SECOND + SECOND / 2, Ordering::Relaxed);
    stats.add("req", vec![], 1);

    let batch = stats.drain(false);
    let series = batch.series("req", &vec![]).unwrap();
    assert_eq!(series.kind, Kind::Counter);
    assert_eq!(series.encoding, Encoding::Count);
    assert_eq!(series.points.counts(), Some(&[(SECOND, 5)][..]));

    let rest = stats.drain(true);
    let series = rest.series("req", &vec![]).unwrap();
    assert_eq!(series.points.counts(), Some(&[(2 * SECOND, 1)][..]));
    assert!(stats.drain(true).is_empty());
  }

  #[test]
  fn gauge_keeps_last_and_refuses_an_encoding_change() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    stats.gauge("depth", vec![], 1i64);
    stats.gauge("depth", vec![], 2i64);
    stats.gauge("depth", vec![], NumberValue::count(3));
    assert_eq!(stats.refused(), 1);
    assert_eq!(
      stats.last_refusal().unwrap().reason,
      RefusalReason::EncodingChanged {
        was: Encoding::Int,
        now: Encoding::Count
      }
    );
    let batch = stats.drain(true);
    let series = batch.series("depth", &vec![]).unwrap();
    assert_eq!(series.points.numbers(), Some(&[(SECOND, NumberValue::from(2i64))][..]));
  }

  #[test]
  fn a_sample_that_does_not_fit_its_kind_is_refused() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    stats.record(Observation {
      name: "x".into(),
      kind: Kind::Counter,
      tags: vec![],
      at_ns: SECOND,
      sample: Sample::Number(1u8.into()),
      resolution: None,
    });
    stats.incr("x", vec![]);
    stats.gauge("x", vec![], 1u8);
    assert_eq!(stats.refused(), 2);
    assert_eq!(
      stats.last_refusal().unwrap().reason,
      RefusalReason::KindChanged {
        was: Kind::Counter,
        now: Kind::Gauge
      }
    );
  }

  #[test]
  fn timers_become_histograms() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    for ms in [5u64, 10, 20] {
      stats.timer("latency", vec![], Duration::from_millis(ms));
    }
    let batch = stats.drain(true);
    let series = batch.series("latency", &vec![]).unwrap();
    assert_eq!(series.encoding, Encoding::Histogram);
    let histograms = series.points.histograms().unwrap();
    assert_eq!(histograms.len(), 1);
    assert_eq!(histograms[0].1.count(), 3);
    assert_eq!(histograms[0].1.max(), Duration::from_millis(20));
  }

  #[test]
  fn totals_become_deltas_and_a_drop_is_a_restart() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    stats.total("ops".into(), vec![], 10);
    assert!(stats.drain(true).is_empty());
    stats.total("ops".into(), vec![], 15);
    stats.total("ops".into(), vec![], 3);
    let batch = stats.drain(true);
    let series = batch.series("ops", &vec![]).unwrap();
    assert_eq!(series.points.counts(), Some(&[(SECOND, 8)][..]));
  }

  #[test]
  fn common_tags_join_the_series_key_and_leave_the_series() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, tags!("env" => "prod"));
    stats.incr("a", tags!("k" => "v", "env" => "prod"));
    stats.incr("a", tags!("k" => "v"));
    let batch = stats.drain(true);
    assert_eq!(batch.common_tags, tags!("env" => "prod"));
    assert_eq!(batch.series.len(), 1);
    assert_eq!(batch.series[0].tags, tags!("k" => "v"));
    assert_eq!(batch.series[0].points.counts(), Some(&[(SECOND, 2)][..]));
  }

  #[test]
  fn observation_tags_win_over_context_tags() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    let context = stats.context(tags!("app" => "x", "k" => "ctx"));
    context.incr("a", tags!("k" => "obs"));
    let batch = stats.drain(true);
    assert_eq!(batch.series[0].tags, vec![Tag::new("app", "x"), Tag::new("k", "obs")]);
  }

  #[test]
  fn collectors_run_on_demand_with_their_tags() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    let id = stats.add_collector_with_tags(
      tags!("pool" => "rpc"),
      Duration::from_secs(3600),
      crate::collect::gauge("pool.active", vec![], || NumberValue::count(7)),
    );
    stats.collect_now();
    let batch = stats.drain(true);
    let series = batch.series("pool.active", &tags!("pool" => "rpc")).unwrap();
    assert_eq!(series.points.numbers(), Some(&[(SECOND, NumberValue::count(7))][..]));
    assert!(stats.remove_collector(id));
    assert!(!stats.remove_collector(id));
    stats.stop();
  }

  #[test]
  fn collect_due_follows_the_clock_and_survives_it_stepping_back() {
    let clock = clock_at(100 * SECOND);
    let stats = stats_with(&clock, vec![]);
    stats.add_collector(
      Duration::from_secs(10),
      crate::collect::gauge("level", vec![], || NumberValue::count(1)),
    );
    stats.collect_due();
    assert!(stats.drain(true).is_empty());

    clock.0.store(110 * SECOND, Ordering::Relaxed);
    stats.collect_due();
    assert_eq!(stats.drain(true).series.len(), 1);

    clock.0.store(50 * SECOND, Ordering::Relaxed);
    stats.collect_due();
    assert!(stats.drain(true).is_empty());
    clock.0.store(60 * SECOND, Ordering::Relaxed);
    stats.collect_due();
    assert_eq!(stats.drain(true).series.len(), 1);
    stats.stop();
  }

  #[test]
  fn stop_returns_with_an_idle_collect_thread() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    let id = stats.add_collector(Duration::from_secs(3600), |_: &dyn Recorder| {});
    assert!(stats.remove_collector(id));
    stats.stop();
  }

  #[test]
  fn stop_hands_everything_to_the_sink() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    let sink = Arc::new(VecSink::new());
    stats.start(sink.clone());
    stats.incr("a", vec![]);
    stats.stop();
    let batches = sink.take();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].series[0].points.counts(), Some(&[(SECOND, 1)][..]));
  }

  #[test]
  fn dropping_the_last_handle_flushes() {
    let clock = clock_at(SECOND);
    let sink = Arc::new(VecSink::new());
    {
      let stats = stats_with(&clock, vec![]);
      stats.set_sink(sink.clone());
      stats.incr("a", vec![]);
    }
    assert_eq!(sink.len(), 1);
  }

  #[cfg(feature = "high_res")]
  #[test]
  fn high_res_samples_are_kept_and_overflow_is_counted() {
    let clock = clock_at(SECOND);
    let stats = stats_with(&clock, vec![]);
    for i in 0..3u64 {
      stats.record(Observation {
        name: "tick".into(),
        kind: Kind::Timer,
        tags: vec![],
        at_ns: SECOND + i,
        sample: Sample::Duration(Duration::from_micros(i)),
        resolution: Some(Duration::from_millis(1)),
      });
    }
    let batch = stats.drain(true);
    let samples = batch
      .series
      .iter()
      .find(|s| s.resolution == Duration::from_millis(1))
      .unwrap();
    assert_eq!(samples.points.samples().unwrap().len(), 2);
    assert_eq!(batch.dropped.len(), 1);
    assert_eq!(batch.dropped[0].count, 1);
    assert_eq!(batch.dropped[0].from_ns, SECOND);
    let aggregated = batch
      .series
      .iter()
      .find(|s| s.resolution == Duration::from_secs(1))
      .unwrap();
    assert_eq!(aggregated.points.histograms().unwrap()[0].1.count(), 3);
  }
}
