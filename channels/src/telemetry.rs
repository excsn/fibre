#[cfg(feature = "diagnostics")]
pub mod enabled {
  use std::cell::UnsafeCell;
  use std::collections::HashMap;
  use std::fmt;
  use std::sync::atomic::{AtomicUsize, Ordering};
  use parking_lot::Mutex;
  use std::thread::{self, ThreadId};
  use std::time::Instant;
  use tokio::task::Id as TokioTaskId;

  static NEXT_EVENT_SEQUENCE_ID: AtomicUsize = AtomicUsize::new(0);

  // Used by the regular (low-frequency) log_event path.
  #[derive(Clone)]
  pub struct TelemetryEvent {
    pub seq_id: usize,
    pub timestamp: Instant,
    pub os_thread_id: ThreadId,
    pub tokio_task_id: Option<TokioTaskId>,
    pub item_id: Option<usize>,
    pub location: String,
    pub event_type: String,
    pub message: Option<String>,
  }

  impl fmt::Debug for TelemetryEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      f.debug_struct("TelemetryEvent")
        .field("seq", &self.seq_id)
        .field("os_tid", &self.os_thread_id)
        .field(
          "tokio_tid",
          &self
            .tokio_task_id
            .map(|id| id.to_string())
            .as_deref()
            .unwrap_or("N/A"),
        )
        .field("item_id", &self.item_id)
        .field("loc", &self.location)
        .field("evt", &self.event_type)
        .field("msg", &self.message.as_deref().unwrap_or(""))
        .finish()
    }
  }

  // --- Zero-barrier stall flight recorder ------------------------------------
  //
  // Each thread owns one slot in TRACES and writes with only Relaxed atomics
  // and plain UnsafeCell stores. No Mutex, no String, no allocator call.
  // This is essential for catching Heisenbugs: a Mutex or heap allocation here
  // would emit the SeqCst barriers that mask the very races we want to observe.

  const MAX_THREADS: usize = 32;
  const STALL_RING_CAP: usize = 128;

  // Fields are all Copy primitives — no allocation, no drop, no barrier.
  #[derive(Clone, Copy)]
  struct StallRecord {
    seq_id: usize,
    location: &'static str,
    event_type: &'static str,
    state_val: usize,
    head: usize,
    tail: usize,
  }

  impl StallRecord {
    const fn empty() -> Self {
      Self { seq_id: 0, location: "", event_type: "", state_val: 0, head: 0, tail: 0 }
    }
  }

  struct ThreadTrace {
    cursor: AtomicUsize,
    records: [UnsafeCell<StallRecord>; STALL_RING_CAP],
  }

  // SAFETY: Each ThreadTrace slot is owned by exactly one thread (determined
  // by MY_TRACE_IDX). Only that thread writes; the dump path reads only after
  // a stall (quiescent state), so there is no concurrent write+read in practice.
  unsafe impl Send for ThreadTrace {}
  unsafe impl Sync for ThreadTrace {}

  impl ThreadTrace {
    const fn new() -> Self {
      Self {
        cursor: AtomicUsize::new(0),
        records: [const { UnsafeCell::new(StallRecord::empty()) }; STALL_RING_CAP],
      }
    }

    fn clear(&self) {
      self.cursor.store(0, Ordering::Relaxed);
    }
  }

  static NEXT_THREAD_ID: AtomicUsize = AtomicUsize::new(0);
  static TRACES: [ThreadTrace; MAX_THREADS] = [const { ThreadTrace::new() }; MAX_THREADS];

  thread_local! {
    static MY_TRACE_IDX: usize =
      NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed) % MAX_THREADS;
  }

  // -------------------------------------------------------------------------

  type CounterKey = (String, String); // (location, counter_name)

  struct CollectorData {
    events: Vec<TelemetryEvent>,
    counters: HashMap<CounterKey, usize>,
    start_time: Instant,
  }

  impl CollectorData {
    fn new() -> Self {
      CollectorData {
        events: Vec::new(),
        counters: HashMap::new(),
        start_time: Instant::now(),
      }
    }
  }

  lazy_static::lazy_static! {
      static ref GLOBAL_COLLECTOR: Mutex<CollectorData> = Mutex::new(CollectorData::new());
  }

  fn record_event_internal(
    item_id: Option<usize>,
    location: &str,
    event_type: &str,
    message: Option<String>,
  ) {
    let tokio_task_id = tokio::task::try_id();
    let event = TelemetryEvent {
      seq_id: NEXT_EVENT_SEQUENCE_ID.fetch_add(1, Ordering::Relaxed),
      timestamp: Instant::now(),
      os_thread_id: thread::current().id(),
      tokio_task_id,
      item_id,
      location: location.to_string(),
      event_type: event_type.to_string(),
      message,
    };
    GLOBAL_COLLECTOR.lock().events.push(event);
  }

  fn record_stall_event_internal(
    location: &'static str,
    event_type: &'static str,
    state_val: usize,
    head: usize,
    tail: usize,
  ) {
    MY_TRACE_IDX.with(|&idx| {
      let trace = &TRACES[idx];
      let c = trace.cursor.load(Ordering::Relaxed);
      let seq = NEXT_EVENT_SEQUENCE_ID.fetch_add(1, Ordering::Relaxed);
      // SAFETY: only this thread writes to slot `c % STALL_RING_CAP`.
      unsafe {
        *trace.records[c % STALL_RING_CAP].get() =
          StallRecord { seq_id: seq, location, event_type, state_val, head, tail };
      }
      trace.cursor.store(c.wrapping_add(1), Ordering::Relaxed);
    });
  }

  fn increment_counter_internal(location: &str, counter_name: &str) {
    let key = (location.to_string(), counter_name.to_string());
    *GLOBAL_COLLECTOR.lock().counters.entry(key).or_insert(0) += 1;
  }

  fn print_report_internal() {
    let collector = GLOBAL_COLLECTOR.lock();
    println!("\n--- Fibre Telemetry Report (Feature: diagnostics) ---");
    println!("Report generated at: {:?}", Instant::now());
    println!("Collection started at: {:?}", collector.start_time);

    if collector.events.is_empty() {
      println!("\n[Events] No detailed events recorded.");
    } else {
      println!("\n[Events] Recorded Events ({}):", collector.events.len());
      let mut sorted_events = collector.events.clone();
      sorted_events.sort_by_key(|e| e.seq_id);
      for event in sorted_events.iter() {
        let time_since_start = event.timestamp.duration_since(collector.start_time);
        let os_tid_short = format!("{:?}", event.os_thread_id)
          .trim_start_matches("ThreadId(")
          .trim_end_matches(')')
          .to_string();
        let tokio_tid_str = event
          .tokio_task_id
          .map(|id| id.to_string())
          .unwrap_or_else(|| "---".to_string());
        println!(
          "  +{:<10.6}s [Seq:{:<5}] OS_TID:{:<6} TaskID:{:<6} Item:{:<6} Loc:{:<25} Evt:{:<30} Msg: {}",
          time_since_start.as_secs_f64(),
          event.seq_id,
          os_tid_short,
          tokio_tid_str,
          event.item_id.map_or_else(|| "N/A".to_string(), |id| id.to_string()),
          event.location,
          event.event_type,
          event.message.as_deref().unwrap_or("")
        );
      }
    }

    if collector.counters.is_empty() {
      println!("\n[Counters] No counters recorded.");
    } else {
      println!(
        "\n[Counters] Recorded Counters ({}):",
        collector.counters.len()
      );
      let mut sorted_counters: Vec<_> = collector.counters.iter().collect();
      sorted_counters.sort_by_key(|(k, _v)| *k);
      for ((loc, name), count) in sorted_counters {
        println!("  Loc:{:<25} Counter:{:<30} Value: {}", loc, name, count);
      }
    }
    println!("\n--- End of Telemetry Report ---");
  }

  fn print_stall_report_internal() {
    println!("\n--- Fibre Stall Diagnostics (Zero-Barrier Flight Recorder) ---");
    let mut all: Vec<(usize, StallRecord)> = Vec::new();
    for (tid, trace) in TRACES.iter().enumerate() {
      let c = trace.cursor.load(Ordering::Relaxed);
      if c == 0 {
        continue;
      }
      let start = c.saturating_sub(STALL_RING_CAP);
      for i in start..c {
        // SAFETY: reading during a quiescent dump; the owning thread is stalled.
        let r = unsafe { *trace.records[i % STALL_RING_CAP].get() };
        if !r.location.is_empty() {
          all.push((tid, r));
        }
      }
    }
    all.sort_by_key(|(_, r)| r.seq_id);
    if all.is_empty() {
      println!("  (no events recorded)");
    } else {
      for (tid, r) in &all {
        println!(
          "  [Seq:{:<6}] T:{:<2} {:<35} {:<25} state={:<5} head={:<6} tail={:<6}",
          r.seq_id, tid, r.location, r.event_type, r.state_val, r.head, r.tail
        );
      }
    }
    println!("--- End ({} events across {} threads) ---\n", all.len(), MAX_THREADS);
  }

  fn clear_data_internal() {
    let mut collector = GLOBAL_COLLECTOR.lock();
    collector.events.clear();
    collector.counters.clear();
    collector.start_time = Instant::now();
    for trace in TRACES.iter() {
      trace.clear();
    }
    NEXT_EVENT_SEQUENCE_ID.store(0, Ordering::Relaxed);
  }

  // --- Public Instrumentation Functions ---

  pub fn log_event_fn(
    item_id: Option<usize>,
    location: &str,
    event_type: &str,
    message: Option<String>,
  ) {
    record_event_internal(item_id, location, event_type, message);
  }

  pub fn log_stall_event_fn(
    location: &'static str,
    event_type: &'static str,
    state_val: usize,
    head: usize,
    tail: usize,
  ) {
    record_stall_event_internal(location, event_type, state_val, head, tail);
  }

  pub fn increment_counter_fn(location: &str, counter_name: &str) {
    increment_counter_internal(location, counter_name);
  }

  pub fn print_telemetry_report_fn() {
    print_report_internal();
  }

  pub fn print_stall_report_fn() {
    print_stall_report_internal();
  }

  pub fn clear_telemetry_fn() {
    clear_data_internal();
  }
} // mod enabled

#[cfg(not(feature = "diagnostics"))]
pub mod disabled {
  #[inline(always)]
  pub fn log_event_fn(
    _item_id: Option<usize>,
    _location: &str,
    _event_type: &str,
    _message: Option<String>,
  ) {
  }
  #[inline(always)]
  pub fn log_stall_event_fn(
    _location: &'static str,
    _event_type: &'static str,
    _state_val: usize,
    _head: usize,
    _tail: usize,
  ) {
  }
  #[inline(always)]
  pub fn increment_counter_fn(_location: &str, _counter_name: &str) {}
  #[inline(always)]
  pub fn print_telemetry_report_fn() {}
  #[inline(always)]
  pub fn print_stall_report_fn() {}
  #[inline(always)]
  pub fn clear_telemetry_fn() {}
}

// log_stall! — zero-barrier flight recorder. Arguments are all integers
// (no format!, no String) so no allocator call ever reaches the macro body.
#[macro_export]
macro_rules! log_stall {
  ($location:expr, $event_type:expr, $state:expr, $head:expr, $tail:expr) => {
    #[cfg(feature = "diagnostics")]
    $crate::telemetry::log_stall_event(
      $location,
      $event_type,
      $state as usize,
      $head as usize,
      $tail as usize,
    );
  };
}

#[macro_export]
macro_rules! log_event {
  ($item_id:expr, $location:expr, $event_type:expr) => {
    #[cfg(feature = "diagnostics")]
    $crate::telemetry::log_event($item_id, $location, $event_type, None);
  };
  ($item_id:expr, $location:expr, $event_type:expr, $($arg:tt)+) => {
    #[cfg(feature = "diagnostics")]
    $crate::telemetry::log_event(
      $item_id,
      $location,
      $event_type,
      Some(format!($($arg)+)),
    );
  };
}

#[macro_export]
macro_rules! increment_counter {
  ($location:expr, $counter_name:expr) => {
    #[cfg(feature = "diagnostics")]
    $crate::telemetry::increment_counter($location, $counter_name);
  };
}

// Re-export the correct set of functions based on the feature flag
#[cfg(feature = "diagnostics")]
pub use enabled::{
  clear_telemetry_fn as clear_telemetry,
  increment_counter_fn as increment_counter,
  log_event_fn as log_event,
  log_stall_event_fn as log_stall_event,
  print_telemetry_report_fn as print_telemetry_report,
  print_stall_report_fn as print_stall_report,
};

#[cfg(not(feature = "diagnostics"))]
pub use disabled::{
  clear_telemetry_fn as clear_telemetry,
  increment_counter_fn as increment_counter,
  log_event_fn as log_event,
  log_stall_event_fn as log_stall_event,
  print_telemetry_report_fn as print_telemetry_report,
  print_stall_report_fn as print_stall_report,
};
