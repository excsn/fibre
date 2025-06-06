// src/telemetry.rs

#[cfg(feature = "fibre_telemetry")]
pub mod enabled {
  use std::collections::HashMap;
  use std::fmt;
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Mutex;
  use std::thread::{self, ThreadId};
  use std::time::Instant;
  use tokio::task::Id as TokioTaskId; 

  static NEXT_EVENT_SEQUENCE_ID: AtomicUsize = AtomicUsize::new(0);

  #[derive(Clone)]
  pub struct TelemetryEvent {
    pub seq_id: usize, // A global sequence number for all events
    pub timestamp: Instant,
    pub os_thread_id: ThreadId,
    pub tokio_task_id: Option<TokioTaskId>, // Optional: Tokio's task ID
    pub item_id: Option<usize>,     // Optional ID for the specific data item
    pub location: String,     // Code location (e.g., module::function)
    pub event_type: String,   // User-defined event type (e.g., "SendAttempt", "RecvSuccess")
    pub message: Option<String>,    // Optional human-readable message or details
  }

  impl fmt::Debug for TelemetryEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      f.debug_struct("TelemetryEvent")
        .field("seq", &self.seq_id)
        // .field("ts_offset_micros", &"TODO_CALC_OFFSET") // Placeholder for relative time
        .field("os_tid", &self.os_thread_id)
        .field("tokio_tid", &self.tokio_task_id.map(|id| id.to_string()).as_deref().unwrap_or("N/A"))
        .field("item_id", &self.item_id)
        .field("loc", &self.location)
        .field("evt", &self.event_type)
        .field("msg", &self.message.as_deref().unwrap_or(""))
        .finish()
    }
  }

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

  // Internal function to record an event
  fn record_event_internal(
    item_id: Option<usize>,
    location: &str,
    event_type: &str,
    message: Option<String>,
  ) {
    let tokio_task_id = tokio::task::try_id().map(|id| id);

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
      println!("\n--- Fibre Telemetry Report (Feature: fibre_telemetry) ---");
      println!("Report generated at: {:?}", Instant::now());
      println!("Collection started at: {:?}", collector.start_time);

      if collector.events.is_empty() {
        println!("\n[Events] No detailed events recorded.");
      } else {
        println!("\n[Events] Recorded Events ({}):", collector.events.len());
        let mut sorted_events = collector.events.clone();
        // Sort by sequence ID to ensure chronological order if timestamps are too close
        sorted_events.sort_by_key(|e| e.seq_id); 

        for event in sorted_events.iter() {
          let time_since_start = event.timestamp.duration_since(collector.start_time);
          let os_tid_short = format!("{:?}", event.os_thread_id).trim_start_matches("ThreadId(").trim_end_matches(')').to_string();
          let tokio_tid_str = event.tokio_task_id.map(|id| id.to_string()).unwrap_or_else(|| "---".to_string());

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
        println!("\n[Counters] Recorded Counters ({}):", collector.counters.len());
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

  fn clear_data_internal() {
    if let Ok(mut collector) = GLOBAL_COLLECTOR.lock() {
      collector.events.clear();
      collector.counters.clear();
      collector.start_time = Instant::now();
    } else {
      eprintln!("[TELEMETRY FT-ERROR] Global collector mutex poisoned, cannot clear data.");
    }
    NEXT_EVENT_SEQUENCE_ID.store(0, Ordering::Relaxed);
  }

  // --- Public Instrumentation Functions ---
  // `enter_span_fn` is removed as we are not doing context-based spans anymore.
  // If you want to mark blocks, you'd use two log_event_fn calls.
  // Example: log_event_fn(None, "my_func", "BlockStart", Some("Doing X"));
  //          ...
  //          log_event_fn(None, "my_func", "BlockEnd", Some("Done X"));

  pub fn log_event_fn(
    item_id: Option<usize>,
    location: &str,
    event_type: &str,
    message: Option<String>,
  ) {
    record_event_internal(item_id, location, event_type, message);
  }

  pub fn increment_counter_fn(location: &'static str, counter_name: &str) {
    increment_counter_internal(location, counter_name);
  }

  pub fn print_telemetry_report_fn() {
    print_report_internal();
  }

  pub fn clear_telemetry_fn() {
    clear_data_internal();
  }
} // mod enabled

#[cfg(not(feature = "fibre_telemetry"))]
pub mod disabled {
  // No TelemetrySpanGuard needed
  #[inline(always)]
  pub fn log_event_fn(
    _item_id: Option<usize>,
    _location: &'static str,
    _event_type: &'static str,
    _message: Option<String>,
  ) {
  }
  #[inline(always)]
  pub fn increment_counter_fn(_location: &'static str, _counter_name: &'static str) {}
  #[inline(always)]
  pub fn print_telemetry_report_fn() {}
  #[inline(always)]
  pub fn clear_telemetry_fn() {}
}

// Re-export the correct set of functions based on the feature flag
#[cfg(feature = "fibre_telemetry")]
pub use enabled::{
  clear_telemetry_fn as clear_telemetry,
  // enter_span_fn as enter_span, // Removed
  increment_counter_fn as increment_counter,
  log_event_fn as log_event,
  print_telemetry_report_fn as print_telemetry_report,
  // TelemetrySpanGuard, // Removed
};

#[cfg(not(feature = "fibre_telemetry"))]
pub use disabled::{
  clear_telemetry_fn as clear_telemetry,
  // enter_span_fn as enter_span, // Removed
  increment_counter_fn as increment_counter,
  log_event_fn as log_event,
  print_telemetry_report_fn as print_telemetry_report,
  // TelemetrySpanGuard, // Removed
};