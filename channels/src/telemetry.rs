#[cfg(feature = "fibre_logging")]
pub mod enabled {
  use std::collections::HashMap;
  use std::fmt;
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Mutex;
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

  // Separate record type for the stall ring: location/event_type are always
  // static string literals at every call site, so we skip the two to_string()
  // allocs that TelemetryEvent would require.
  #[derive(Clone)]
  struct StallRecord {
    seq_id: usize,
    timestamp: Instant,
    os_thread_id: ThreadId,
    tokio_task_id: Option<TokioTaskId>,
    item_id: Option<usize>,
    location: &'static str,
    event_type: &'static str,
    message: Option<String>,
  }

  // --- Lock-free stall ring -------------------------------------------------
  //
  // 256 independent per-slot parking_lot::Mutex slots indexed by an atomic
  // cursor. Writers do one fetch_add + one uncontended per-slot lock. Two
  // writers contend only if they land on the same slot mod 256 simultaneously,
  // which is rare and still correct. Reads (dump) iterate all slots
  // independently.

  const STALL_RING_CAP: usize = 256;

  struct StallRing {
    cursor: AtomicUsize,
    slots: Box<[parking_lot::Mutex<Option<StallRecord>>]>,
  }

  // SAFETY: StallRecord contains no thread-affine types; parking_lot::Mutex is
  // Send + Sync.
  unsafe impl Send for StallRing {}
  unsafe impl Sync for StallRing {}

  impl StallRing {
    fn new() -> Self {
      StallRing {
        cursor: AtomicUsize::new(0),
        slots: (0..STALL_RING_CAP)
          .map(|_| parking_lot::Mutex::new(None))
          .collect::<Vec<_>>()
          .into_boxed_slice(),
      }
    }

    #[inline]
    fn push(&self, record: StallRecord) {
      let idx = self.cursor.fetch_add(1, Ordering::Relaxed) & (STALL_RING_CAP - 1);
      // Drops the evicted entry (if any) while holding the per-slot lock.
      *self.slots[idx].lock() = Some(record);
    }

    fn snapshot_sorted(&self) -> Vec<StallRecord> {
      let mut out = Vec::with_capacity(STALL_RING_CAP);
      for slot in self.slots.iter() {
        if let Some(r) = slot.lock().clone() {
          out.push(r);
        }
      }
      out.sort_by_key(|r| r.seq_id);
      out
    }

    fn clear(&self) {
      for slot in self.slots.iter() {
        *slot.lock() = None;
      }
    }
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
      static ref STALL_RING: StallRing = StallRing::new();
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
    if let Ok(mut collector) = GLOBAL_COLLECTOR.lock() {
      collector.events.push(event);
    } else {
      eprintln!("[TELEMETRY FT-ERROR] Global collector mutex poisoned while recording event.");
    }
  }

  fn record_stall_event_internal(
    item_id: Option<usize>,
    location: &'static str,
    event_type: &'static str,
    message: Option<String>,
  ) {
    let record = StallRecord {
      seq_id: NEXT_EVENT_SEQUENCE_ID.fetch_add(1, Ordering::Relaxed),
      timestamp: Instant::now(),
      os_thread_id: thread::current().id(),
      tokio_task_id: tokio::task::try_id(),
      item_id,
      location,
      event_type,
      message,
    };
    STALL_RING.push(record);
  }

  fn increment_counter_internal(location: &str, counter_name: &str) {
    let key = (location.to_string(), counter_name.to_string());
    if let Ok(mut collector) = GLOBAL_COLLECTOR.lock() {
      *collector.counters.entry(key).or_insert(0) += 1;
    } else {
      eprintln!("[TELEMETRY FT-ERROR] Global collector mutex poisoned while incrementing counter.");
    }
  }

  fn print_report_internal() {
    if let Ok(collector) = GLOBAL_COLLECTOR.lock() {
      println!("\n--- Fibre Telemetry Report (Feature: fibre_logging) ---");
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
    } else {
      eprintln!("[TELEMETRY FT-ERROR] Global collector mutex poisoned, cannot print report.");
    }
  }

  fn print_stall_report_internal() {
    let start_time = match GLOBAL_COLLECTOR.lock() {
      Ok(c) => c.start_time,
      Err(_) => Instant::now(),
    };
    let records = STALL_RING.snapshot_sorted();

    println!(
      "\n--- Fibre Stall Diagnostics Report (Last {} Events) ---",
      STALL_RING_CAP
    );
    println!("Report generated at: {:?}", Instant::now());

    if records.is_empty() {
      println!("\n[Stall Events] No recent stall diagnostic events recorded.");
    } else {
      for r in &records {
        let time_since_start = r.timestamp.duration_since(start_time);
        let os_tid_short = format!("{:?}", r.os_thread_id)
          .trim_start_matches("ThreadId(")
          .trim_end_matches(')')
          .to_string();
        let tokio_tid_str = r
          .tokio_task_id
          .map(|id| id.to_string())
          .unwrap_or_else(|| "---".to_string());
        println!(
          "  +{:<10.6}s [Seq:{:<5}] OS_TID:{:<6} TaskID:{:<6} Item:{:<6} Loc:{:<25} Evt:{:<30} Msg: {}",
          time_since_start.as_secs_f64(),
          r.seq_id,
          os_tid_short,
          tokio_tid_str,
          r.item_id.map_or_else(|| "N/A".to_string(), |id| id.to_string()),
          r.location,
          r.event_type,
          r.message.as_deref().unwrap_or("")
        );
      }
    }
    println!("\n--- End of Stall Diagnostics Report ---");
  }

  fn clear_data_internal() {
    if let Ok(mut collector) = GLOBAL_COLLECTOR.lock() {
      collector.events.clear();
      collector.counters.clear();
      collector.start_time = Instant::now();
    } else {
      eprintln!("[TELEMETRY FT-ERROR] Global collector mutex poisoned, cannot clear data.");
    }
    STALL_RING.clear();
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
    item_id: Option<usize>,
    location: &'static str,
    event_type: &'static str,
    message: Option<String>,
  ) {
    record_stall_event_internal(item_id, location, event_type, message);
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

#[cfg(not(feature = "fibre_logging"))]
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
    _item_id: Option<usize>,
    _location: &'static str,
    _event_type: &'static str,
    _message: Option<String>,
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

// Declarative macros — `#[cfg(...)]` inside each arm prevents eager argument
// evaluation (including format! calls) when fibre_logging is disabled.
#[macro_export]
macro_rules! log_stall {
  ($item_id:expr, $location:expr, $event_type:expr) => {
    #[cfg(feature = "fibre_logging")]
    $crate::telemetry::log_stall_event($item_id, $location, $event_type, None);
  };
  ($item_id:expr, $location:expr, $event_type:expr, $($arg:tt)+) => {
    #[cfg(feature = "fibre_logging")]
    $crate::telemetry::log_stall_event(
      $item_id,
      $location,
      $event_type,
      Some(format!($($arg)+)),
    );
  };
}

#[macro_export]
macro_rules! log_event {
  ($item_id:expr, $location:expr, $event_type:expr) => {
    #[cfg(feature = "fibre_logging")]
    $crate::telemetry::log_event($item_id, $location, $event_type, None);
  };
  ($item_id:expr, $location:expr, $event_type:expr, $($arg:tt)+) => {
    #[cfg(feature = "fibre_logging")]
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
    #[cfg(feature = "fibre_logging")]
    $crate::telemetry::increment_counter($location, $counter_name);
  };
}

// Re-export the correct set of functions based on the feature flag
#[cfg(feature = "fibre_logging")]
pub use enabled::{
  clear_telemetry_fn as clear_telemetry,
  increment_counter_fn as increment_counter,
  log_event_fn as log_event,
  log_stall_event_fn as log_stall_event,
  print_telemetry_report_fn as print_telemetry_report,
  print_stall_report_fn as print_stall_report,
};

#[cfg(not(feature = "fibre_logging"))]
pub use disabled::{
  clear_telemetry_fn as clear_telemetry,
  increment_counter_fn as increment_counter,
  log_event_fn as log_event,
  log_stall_event_fn as log_stall_event,
  print_telemetry_report_fn as print_telemetry_report,
  print_stall_report_fn as print_stall_report,
};
